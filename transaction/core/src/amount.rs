// Copyright (c) 2018-2020 MobileCoin Inc.

//! A commitment to an output's amount.
//!
//! Amounts are implemented as Pedersen commitments. The associated private keys are "masked" using
//! a shared secret.

#![cfg_attr(test, allow(clippy::unnecessary_operation))]

use crate::{
    constants::MAX_TINY_MOB,
    ring_signature::{Blinding, Commitment, CurveScalar, GENERATORS},
};
use blake2::{Blake2b, Digest};
use curve25519_dalek::scalar::Scalar;
use digestible::Digestible;
use failure::Fail;
use keys::RistrettoPublic;
use mcserial::ReprBytes32;
use prost::Message;
use serde::{Deserialize, Serialize};

/// Errors that can occur when constructing an amount.
#[derive(Debug, Fail, Eq, PartialEq)]
pub enum AmountError {
    /// The Amount is too damn high.
    #[fail(display = "Amount exceeds MAX_TINY_MOB: {}", _0)]
    ExceedsLimit(u64),

    /// The masked value, masked blinding, or shared secret are not consistent with the commitment.
    #[fail(display = "Inconsistent Commitment")]
    InconsistentCommitment,
}

/// A commitment to the amount of the `n^th` output in a transaction.
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Message, Digestible)]
pub struct Amount {
    /// A Pedersen commitment `v*G + b*H` to a quantity `v` of MobileCoin, with blinding `b`,
    #[prost(message, required, tag = "1")]
    pub commitment: Commitment,

    /// `masked_value = value + Blake2B(shared_secret)`
    #[prost(message, required, tag = "2")]
    pub masked_value: CurveScalar,

    /// `masked_blinding = blinding + Blake2B(Blake2B(shared_secret))
    #[prost(message, required, tag = "3")]
    pub masked_blinding: Blinding,
}

impl Amount {
    /// Creates a commitment `value*G + blinding*H`, and "masks" the commitment secrets
    /// so that they can be recovered by the recipient.
    ///
    /// # Arguments
    /// * `value` - The committed value `v`.
    /// * `blinding` - The blinding `b`.
    /// * `shared_secret` - The shared secret, e.g. `rB` for transaction private key `r` and recipient public key `B`.
    #[inline]
    pub fn new(
        value: u64,
        blinding: Blinding,
        shared_secret: &RistrettoPublic,
    ) -> Result<Amount, AmountError> {
        if value > MAX_TINY_MOB {
            return Err(AmountError::ExceedsLimit(value));
        }

        let value: Scalar = Scalar::from(value);

        // Pedersen commitment `v*G + b*H`.
        let commitment: Commitment = Commitment::from(GENERATORS.commit(value, blinding.into()));

        // `v + Blake2B(shared_secret)`
        let masked_value: Scalar = {
            let mask = get_value_mask(&shared_secret);
            value + mask
        };

        // `s + Blake2B(Blake2B(shared_secret))`
        let masked_blinding: Scalar = {
            let mask = get_blinding_mask(&shared_secret);
            blinding.as_ref() + mask
        };

        Ok(Amount {
            commitment,
            masked_blinding: Blinding::from(masked_blinding),
            masked_value: CurveScalar::from(masked_value),
        })
    }

    /// Returns the value `v` and blinding `b` in the commitment `v*G + b*H`.
    ///
    /// # Arguments
    /// * `shared_secret` - The shared secret, e.g. `rB`.
    pub fn get_value(
        &self,
        shared_secret: &RistrettoPublic,
    ) -> Result<(u64, Blinding), AmountError> {
        let value: u64 = self.unmask_value(shared_secret);
        let blinding = self.unmask_blinding(shared_secret);

        let expected_commitment =
            Commitment::from(GENERATORS.commit(Scalar::from(value), blinding.into()));
        if self.commitment != expected_commitment {
            // The commitment does not agree with the provided value and blinding.
            // This either means that the commitment does not correspond to the shared secret, or
            // that the amount is malformed (and is probably not spendable).
            return Err(AmountError::InconsistentCommitment);
        }

        Ok((value, blinding))
    }

    /// Reveals `masked_value`.
    fn unmask_value(&self, shared_secret: &RistrettoPublic) -> u64 {
        let mask = get_value_mask(shared_secret);
        let masked_value: Scalar = self.masked_value.into();
        let value_as_scalar = masked_value - mask;
        // TODO: better way to do this?
        // We might want to give an error if scalar.as_bytes() is larger than u64
        let mut temp = [0u8; 8];
        temp.copy_from_slice(&value_as_scalar.as_bytes()[0..8]);
        // Note: Dalek documents that scalar.as_bytes() returns in little-endian
        // https://doc.dalek.rs/curve25519_dalek/scalar/struct.Scalar.html#method.as_bytes
        u64::from_le_bytes(temp)
    }

    /// Reveals masked_blinding.
    fn unmask_blinding(&self, shared_secret: &RistrettoPublic) -> Blinding {
        let mask = get_blinding_mask(shared_secret);
        let masked_blinding: Scalar = self.masked_blinding.into();
        Blinding::from(masked_blinding - mask)
    }
}

/// Computes `Blake2B(shared_secret)`
///
/// # Arguments
/// * `shared_secret` - The shared secret, e.g. `rB`.
fn get_value_mask(shared_secret: &RistrettoPublic) -> Scalar {
    get_mask(&shared_secret)
}

/// Computes `Blake2B(Blake2B(shared_secret)`.
///
/// # Arguments
/// * `shared_secret` - The shared secret, e.g. `rB`.
fn get_blinding_mask(shared_secret: &RistrettoPublic) -> Scalar {
    let inner_mask = get_mask(shared_secret);

    let mut hasher = Blake2b::new();
    hasher.input(&inner_mask.to_bytes());

    Scalar::from_hash(hasher)
}

/// Computes `Blake2B(shared_secret)`.
fn get_mask(shared_secret: &RistrettoPublic) -> Scalar {
    let mut hasher = Blake2b::new();
    hasher.input(&shared_secret.to_bytes());
    Scalar::from_hash(hasher)
}

#[cfg(test)]
mod tests {
    use crate::{proptest_fixtures::*, ring_signature::Commitment};
    use proptest::prelude::*;

    use crate::{
        amount::{Amount, AmountError},
        constants::MAX_TINY_MOB,
        ring_signature::{Scalar, GENERATORS},
    };

    proptest! {

            #[test]
            /// Amount::new() should return Ok for valid values and blindings.
            fn test_new_ok(
                value in (0u64..=MAX_TINY_MOB),
                blinding in arbitrary_blinding(),
                shared_secret in arbitrary_ristretto_public()) {
                assert!(Amount::new(value, blinding, &shared_secret).is_ok());
            }

            #[test]
            /// Amount::new() should return ExceedsLimit for values larger than MAX_TINY_MOB.
            fn test_new_exceeds_limit_error(
                value in ((MAX_TINY_MOB+1)..=core::u64::MAX),
                blinding in arbitrary_blinding(),
                shared_secret in arbitrary_ristretto_public()) {

                match Amount::new(value, blinding, &shared_secret){
                    Err(AmountError::ExceedsLimit(_)) => {}, // This is expected.
                    _ => panic!(),
                }
            }

            #[test]
            #[allow(non_snake_case)]
            /// amount.commitment should agree with the value and blinding.
            fn test_commitment(
                value in (0u64..=MAX_TINY_MOB),
                blinding in arbitrary_blinding(),
                shared_secret in arbitrary_ristretto_public()) {
                    let amount = Amount::new(value, blinding,  &shared_secret).unwrap();
                    let G = GENERATORS.B;
                    let H = GENERATORS.B_blinding;

                    let blinding: Scalar = blinding.into();
                    let expected_commitment: Commitment = Commitment::from(Scalar::from(value) * G + blinding * H);
                    assert_eq!(amount.commitment, expected_commitment);
            }

            #[test]
            /// amount.unmask_value should return the value used to construct the amount.
            fn test_unmask_value(
                value in (0u64..=MAX_TINY_MOB),
                blinding in arbitrary_blinding(),
                shared_secret in arbitrary_ristretto_public())
            {

                let amount = Amount::new(value, blinding,  &shared_secret).unwrap();
                assert_eq!(
                    value,
                    amount.unmask_value(&shared_secret)
                );
            }

            #[test]
            /// amount.unmask_blinding should return the blinding used to construct the amount.
            fn test_unmask_blinding(
                value in (0u64..=MAX_TINY_MOB),
                blinding in arbitrary_blinding(),
                shared_secret in arbitrary_ristretto_public())
            {
                let amount = Amount::new(value, blinding,  &shared_secret).unwrap();
                assert_eq!(
                    amount.unmask_blinding(&shared_secret),
                    blinding
                );
            }

            #[test]
            /// get_value should return the correct value and blinding.
            fn test_get_value_ok(
                value in (0u64..=MAX_TINY_MOB),
                blinding in arbitrary_blinding(),
                shared_secret in arbitrary_ristretto_public()) {

                let amount = Amount::new(value, blinding,  &shared_secret).unwrap();
                let result = amount.get_value(&shared_secret);
                let expected = Ok((value, blinding));
                assert_eq!(result, expected);
            }


            #[test]
            /// get_value should return InconsistentCommitment if the masked value is incorrect.
            fn test_get_value_incorrect_masked_value(
                value in (0u64..=MAX_TINY_MOB),
                other_masked_value in arbitrary_curve_scalar(),
                blinding in arbitrary_blinding(),
                shared_secret in arbitrary_ristretto_public())
            {
                // Mutate amount to use a different masked value.
                // With overwhelming probability, amount.masked_value won't equal other_masked_value.
                let mut amount = Amount::new(value, blinding, &shared_secret).unwrap();
                amount.masked_value = other_masked_value;
                let result = amount.get_value(&shared_secret);
                let expected = Err(AmountError::InconsistentCommitment);
                assert_eq!(result, expected);
            }

            #[test]
            /// get_value should return InconsistentCommitment if the masked blinding is incorrect.
            fn test_get_value_incorrect_blinding(
                value in (0u64..=MAX_TINY_MOB),
                blinding in arbitrary_blinding(),
                 other_masked_blinding in arbitrary_curve_scalar(),
                shared_secret in arbitrary_ristretto_public())
            {
                // Mutate amount to use a other_masked_blinding.
                let mut amount = Amount::new(value, blinding, &shared_secret).unwrap();
                amount.masked_blinding = other_masked_blinding;
                let result = amount.get_value(&shared_secret);
                let expected = Err(AmountError::InconsistentCommitment);
                assert_eq!(result, expected);
            }

            #[test]
            /// get_value should return an Error if shared_secret is incorrect.
            fn test_get_value_invalid_shared_secret(
                value in (0u64..=MAX_TINY_MOB),
                blinding in arbitrary_blinding(),
                shared_secret in arbitrary_ristretto_public(),
                other_shared_secret in arbitrary_ristretto_public(),
            ) {
                let amount = Amount::new(value, blinding,  &shared_secret).unwrap();
                let result = amount.get_value(&other_shared_secret);
                let expected = Err(AmountError::InconsistentCommitment);
                assert_eq!(result, expected);
            }
    }
}
