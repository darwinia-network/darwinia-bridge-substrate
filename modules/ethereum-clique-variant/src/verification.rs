// Copyright 2019-2021 Parity Technologies (UK) Ltd.
// This file is part of Parity Bridges Common.

// Parity Bridges Common is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Parity Bridges Common is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Parity Bridges Common.  If not, see <http://www.gnu.org/licenses/>.

use crate::error::Error;
use crate::validators::{Validators, ValidatorsConfiguration};
use crate::{
	ChainTime, CliqueVariantConfiguration, CliqueVariantScheduledChange, ImportContext, PoolConfiguration, Storage,
};
use bp_eth_clique::{
	public_to_address, step_validator, Address, CliqueHeader, HeaderId, ADDRESS_LENGTH, DIFF_INTURN, DIFF_NOTURN, H256,
	H520, KECCAK_EMPTY_LIST_RLP, SIGNATURE_LENGTH, U128, U256, VANITY_LENGTH,
};
use codec::Encode;
use sp_io::crypto::secp256k1_ecdsa_recover;
use sp_runtime::transaction_validity::TransactionTag;
use sp_std::{vec, vec::Vec};

/// Pre-check to see if should try and import this header.
/// Returns error if we should not try to import this block.
/// Returns ID of passed header and best finalized header.
pub fn is_importable_header<S: Storage>(storage: &S, header: &CliqueHeader) -> Result<(HeaderId, HeaderId), Error> {
	// we never import any header that competes with finalized header
	let finalized_id = storage.finalized_block();
	if header.number <= finalized_id.number {
		return Err(Error::AncientHeader);
	}
	// we never import any header with known hash
	let id = header.compute_id();
	if storage.header(&id.hash).is_some() {
		return Err(Error::KnownHeader);
	}

	Ok((id, finalized_id))
}

/// Try accept unsigned clique header into transaction pool.
/// Returns required and provided tags.
pub fn accept_clique_header_into_pool<S: Storage, CT: ChainTime>(
	storage: &S,
	config: &CliqueVariantConfiguration,
	pool_config: &PoolConfiguration,
	header: &CliqueHeader,
	chain_time: &CT,
) -> Result<(Vec<TransactionTag>, Vec<TransactionTag>), Error> {
	// check if we can verify further
	let (header_id, _) = is_importable_header(storage, header)?;

	// we can always do contextless checks
	contextless_checks(config, header, chain_time)?;

	// we do not want to have all future headers in the pool at once
	// => if we see header with number > maximal ever seen header number + LIMIT,
	// => we consider this transaction invalid, but only at this moment (we do not want to ban it)
	// => let's mark it as Unknown transaction
	let (best_id, _) = storage.best_block();
	let difference = header.number.saturating_sub(best_id.number);
	if difference > pool_config.max_future_number_difference {
		return Err(Error::UnsignedTooFarInTheFuture);
	}

	// depending on whether parent header is available, we either perform full or 'shortened' check
	let context = storage.import_context(None, &header.parent_hash);
	let tags = match context {
		Some(context) => {
			let header_step = contextual_checks(config, &context, None, header)?;
			validator_checks(config, &context.validators_set().validators, header)?;
		}
		None => {
			// we know nothing about parent header
			// => the best thing we can do is to believe that there are no forks in
			// PoA chain AND that the header is produced either by previous, or next
			// scheduled validators set change
			let best_context = storage.import_context(None, &best_id.hash).expect(
				"import context is None only when header is missing from the storage;\
							best header is always in the storage; qed",
			);
			let validators_check_result = validator_checks(config, &best_context.validators_set().validators, header);
			if let Err(error) = validators_check_result {
				find_next_validators_signal(storage, &best_context)
					.ok_or(error)
					.and_then(|next_validators| validator_checks(config, &next_validators, header, header_step))?;
			}

			// since our parent is missing from the storage, we **DO** require it
			// to be in the transaction pool
			// (- 1 can't underflow because there's always best block in the header)
			let requires_header_number_and_hash_tag = HeaderId {
				number: header.number - 1,
				hash: header.parent_hash,
			}
			.encode();
			(
				vec![requires_header_number_and_hash_tag],
				vec![provides_number_and_authority_tag, provides_header_number_and_hash_tag],
			)
		}
	};

	Ok(tags)
}

/// Verify header by CliqueVariant rules.
pub fn verify_clique_variant_header<S: Storage, CT: ChainTime>(
	storage: &S,
	config: &CliqueVariantConfiguration,
	submitter: Option<S::Submitter>,
	header: &CliqueHeader,
	chain_time: &CT,
) -> Result<ImportContext<S::Submitter>, Error> {
	// let's do the lightest check first
	contextless_checks(config, header, chain_time)?;

	// the rest of checks requires access to the parent header
	let context = storage.import_context(submitter, &header.parent_hash).ok_or_else(|| {
		log::warn!(
			target: "runtime",
			"Missing parent PoA block: ({:?}, {})",
			header.number.checked_sub(1),
			header.parent_hash,
		);

		Error::MissingParentBlock
	})?;
	let header_step = contextual_checks(config, &context, None, header)?;
	validator_checks(config, &context.validators_set().validators, header, header_step)?;

	Ok(context)
}

/// Perform basic checks that only require header itself.
fn contextless_checks<CT: ChainTime>(
	config: &CliqueVariantConfiguration,
	header: &CliqueHeader,
	chain_time: &CT,
) -> Result<(), Error> {
	// he genesis block is the always valid dead-end
	if header.number == 0 {
		Ok(())
	}
	// Don't waste time checking blocks from the future
	if chain_time.is_timestamp_ahead(header.timestamp) {
		return Err(Error::HeaderTimestampIsAhead);
	}
	// Check that the extra-data contains the vanity, validators and signature.
	if header.extra_data.size() < VANITY_LENGTH {
		return Err(Error::MissingVanity);
	}
	if header.extra_data.size() < VANITY_LENGTH + SIGNATURE_LENGTH {
		return Err(Error::MissingSignature);
	}
	if header.number >= u64::max_value() {
		return Err(Error::RidiculousNumber);
	}
	// Ensure that the extra-data contains a validator list on checkpoint, but none otherwise
	let is_checkpoint = header.number % config.epoch_length == 0;
	let validator_bytes_len = header.extra_data.size() - (VANITY_LENGTH + SIGNATURE_LENGTH);
	if !is_checkpoint && validator_bytes_len != 0 {
		return Err(Error::ExtraValidators);
	}
	// Checkpoint blocks must at least contain one validator
	if is_checkpoint && validator_bytes_len == 0 {
		return Err(Error::InvalidCheckpointValidators);
	}
	// Ensure that the validator bytes length is valid
	if is_checkpoint && validator_bytes_len % ADDRESS_LENGTH {
		return Err(Error::InvalidCheckpointValidators);
	}
	// Ensure that the mix digest is zero as we don't have fork protection currently
	if !header.mix_digest.is_zero() {
		return Err(Error::InvalidMixDigest);
	}
	// Ensure that the block doesn't contain any uncles which are meaningless in PoA
	if header.uncle_hash != KECCAK_EMPTY_LIST_RLP {
		return Err(Error::InvalidUncleHash);
	}
	// Ensure difficulty is valid
	if header.difficulty != DIFF_INTURN && header.difficulty != DIFF_NOTURN {
		return Err(Error::InvalidDifficulty);
	}
	// Ensure that none is empty
	if !header.nonce.is_zero() {
		return Err(Error::InvalidNonce);
	}
	// Ensure that the block's difficulty is meaningful (may not be correct at this point)
	if header.number > 0 && header.Difficulty.is_zero() {
		return Err(Error::InvalidDifficulty);
	}
	if header.gas_used > header.gas_limit {
		return Err(Error::TooMuchGasUsed);
	}
	if header.gas_limit < config.min_gas_limit {
		return Err(Error::InvalidGasLimit);
	}
	if header.gas_limit > config.max_gas_limit {
		return Err(Error::InvalidGasLimit);
	}

	Ok(())
}

/// Perform checks that require access to parent header.
fn contextual_checks<Submitter>(
	config: &CliqueVariantConfiguration,
	context: &ImportContext<Submitter>,
	validators_override: Option<&[Address]>,
	header: &CliqueHeader,
) -> Result<(), Error> {
	let validators = validators_override.unwrap_or_else(|| &context.validators_set().validators);

	// parent sanity check
	if context.parent_hash != header.parent_hash || context.parent_header().number + 1 != header.number {
		return Err(Error::UnknownAncestor);
	}

	// Ensure that the block's timestamp isn't too close to it's parent
	if header.timestamp < context.parent_header().timestamp.saturating_add(config.period) {
		return Err(Error::HeaderTimestampTooClose);
	}

	Ok(())
}

/// Verify that the signature over message has been produced by given validator.
fn verify_signature(expected_validator: &Address, signature: &H520, message: &H256) -> bool {
	secp256k1_ecdsa_recover(signature.as_fixed_bytes(), message.as_fixed_bytes())
		.map(|public| public_to_address(&public))
		.map(|address| *expected_validator == address)
		.unwrap_or(false)
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::mock::{
		insert_header, run_test_with_genesis, test_clique_variant_config, validator, validator_address,
		validators_addresses, validators_change_receipt, AccountId, ConstChainTime, HeaderBuilder, TestRuntime,
		GAS_LIMIT,
	};
	use crate::validators::ValidatorsSource;
	use crate::DefaultInstance;
	use crate::{
		pool_configuration, BridgeStorage, FinalizedBlock, Headers, HeadersByNumber, NextValidatorsSetId,
		ScheduledChanges, ValidatorsSet, ValidatorsSets,
	};
	use bp_eth_clique::{compute_merkle_root, rlp_encode, TransactionOutcome, H520, U256};
	use frame_support::{StorageMap, StorageValue};
	use hex_literal::hex;
	use secp256k1::SecretKey;
	use sp_runtime::transaction_validity::TransactionTag;

	const GENESIS_STEP: u64 = 42;
	const TOTAL_VALIDATORS: usize = 3;

	fn genesis() -> CliqueHeader {
		HeaderBuilder::genesis().step(GENESIS_STEP).sign_by(&validator(0))
	}

	fn verify_with_config(
		config: &CliqueVariantConfiguration,
		header: &CliqueHeader,
	) -> Result<ImportContext<AccountId>, Error> {
		run_test_with_genesis(genesis(), TOTAL_VALIDATORS, |_| {
			let storage = BridgeStorage::<TestRuntime>::new();
			verify_clique_variant_header(&storage, &config, None, header, &ConstChainTime::default())
		})
	}

	fn default_verify(header: &CliqueHeader) -> Result<ImportContext<AccountId>, Error> {
		verify_with_config(&test_clique_variant_config(), header)
	}

	fn default_accept_into_pool(
		mut make_header: impl FnMut(&[SecretKey]) -> (CliqueHeader, Option<Vec<Receipt>>),
	) -> Result<(Vec<TransactionTag>, Vec<TransactionTag>), Error> {
		run_test_with_genesis(genesis(), TOTAL_VALIDATORS, |_| {
			let validators = vec![validator(0), validator(1), validator(2)];
			let mut storage = BridgeStorage::<TestRuntime>::new();
			let block1 = HeaderBuilder::with_parent_number(0).sign_by_set(&validators);
			insert_header(&mut storage, block1);
			let block2 = HeaderBuilder::with_parent_number(1).sign_by_set(&validators);
			let block2_id = block2.compute_id();
			insert_header(&mut storage, block2);
			let block3 = HeaderBuilder::with_parent_number(2).sign_by_set(&validators);
			insert_header(&mut storage, block3);

			FinalizedBlock::<DefaultInstance>::put(block2_id);

			let validators_config =
				ValidatorsConfiguration::Single(ValidatorsSource::Contract(Default::default(), Vec::new()));
			let (header, receipts) = make_header(&validators);
			accept_clique_header_into_pool(
				&storage,
				&test_clique_variant_config(),
				&validators_config,
				&pool_configuration(),
				&header,
				&(),
				receipts.as_ref(),
			)
		})
	}

	fn change_validators_set_at(number: u64, finalized_set: Vec<Address>, signalled_set: Option<Vec<Address>>) {
		let set_id = NextValidatorsSetId::<DefaultInstance>::get();
		NextValidatorsSetId::<DefaultInstance>::put(set_id + 1);
		ValidatorsSets::<DefaultInstance>::insert(
			set_id,
			ValidatorsSet {
				validators: finalized_set,
				signal_block: None,
				enact_block: HeaderId {
					number: 0,
					hash: HeadersByNumber::<DefaultInstance>::get(&0).unwrap()[0],
				},
			},
		);

		let header_hash = HeadersByNumber::<DefaultInstance>::get(&number).unwrap()[0];
		let mut header = Headers::<TestRuntime>::get(&header_hash).unwrap();
		header.next_validators_set_id = set_id;
		if let Some(signalled_set) = signalled_set {
			header.last_signal_block = Some(HeaderId {
				number: header.header.number - 1,
				hash: header.header.parent_hash,
			});
			ScheduledChanges::<DefaultInstance>::insert(
				header.header.parent_hash,
				CliqueVariantScheduledChange {
					validators: signalled_set,
					prev_signal_block: None,
				},
			);
		}

		Headers::<TestRuntime>::insert(header_hash, header);
	}

	#[test]
	fn verifies_seal_count() {
		// when there are no seals at all
		let mut header = CliqueHeader::default();
		assert_eq!(default_verify(&header), Err(Error::InvalidSealArity));

		// when there's single seal (we expect 2 or 3 seals)
		header.seal = vec![vec![]];
		assert_eq!(default_verify(&header), Err(Error::InvalidSealArity));

		// when there's 3 seals (we expect 2 by default)
		header.seal = vec![vec![], vec![], vec![]];
		assert_eq!(default_verify(&header), Err(Error::InvalidSealArity));

		// when there's 2 seals
		header.seal = vec![vec![], vec![]];
		assert_ne!(default_verify(&header), Err(Error::InvalidSealArity));
	}

	#[test]
	fn verifies_header_number() {
		// when number is u64::max_value()
		let header = HeaderBuilder::with_number(u64::max_value()).sign_by(&validator(0));
		assert_eq!(default_verify(&header), Err(Error::RidiculousNumber));

		// when header is < u64::max_value()
		let header = HeaderBuilder::with_number(u64::max_value() - 1).sign_by(&validator(0));
		assert_ne!(default_verify(&header), Err(Error::RidiculousNumber));
	}

	#[test]
	fn verifies_gas_used() {
		// when gas used is larger than gas limit
		let header = HeaderBuilder::with_number(1)
			.gas_used((GAS_LIMIT + 1).into())
			.sign_by(&validator(0));
		assert_eq!(default_verify(&header), Err(Error::TooMuchGasUsed));

		// when gas used is less than gas limit
		let header = HeaderBuilder::with_number(1)
			.gas_used((GAS_LIMIT - 1).into())
			.sign_by(&validator(0));
		assert_ne!(default_verify(&header), Err(Error::TooMuchGasUsed));
	}

	#[test]
	fn verifies_gas_limit() {
		let mut config = test_clique_variant_config();
		config.min_gas_limit = 100.into();
		config.max_gas_limit = 200.into();

		// when limit is lower than expected
		let header = HeaderBuilder::with_number(1)
			.gas_limit(50.into())
			.sign_by(&validator(0));
		assert_eq!(verify_with_config(&config, &header), Err(Error::InvalidGasLimit));

		// when limit is larger than expected
		let header = HeaderBuilder::with_number(1)
			.gas_limit(250.into())
			.sign_by(&validator(0));
		assert_eq!(verify_with_config(&config, &header), Err(Error::InvalidGasLimit));

		// when limit is within expected range
		let header = HeaderBuilder::with_number(1)
			.gas_limit(150.into())
			.sign_by(&validator(0));
		assert_ne!(verify_with_config(&config, &header), Err(Error::InvalidGasLimit));
	}

	#[test]
	fn verifies_extra_data_len() {
		// when extra data is too large
		let header = HeaderBuilder::with_number(1)
			.extra_data(std::iter::repeat(42).take(1000).collect::<Vec<_>>())
			.sign_by(&validator(0));
		assert_eq!(default_verify(&header), Err(Error::ExtraDataOutOfBounds));

		// when extra data size is OK
		let header = HeaderBuilder::with_number(1)
			.extra_data(std::iter::repeat(42).take(10).collect::<Vec<_>>())
			.sign_by(&validator(0));
		assert_ne!(default_verify(&header), Err(Error::ExtraDataOutOfBounds));
	}

	#[test]
	fn verifies_timestamp() {
		// when timestamp overflows i32
		let header = HeaderBuilder::with_number(1)
			.timestamp(i32::max_value() as u64 + 1)
			.sign_by(&validator(0));
		assert_eq!(default_verify(&header), Err(Error::TimestampOverflow));

		// when timestamp doesn't overflow i32
		let header = HeaderBuilder::with_number(1)
			.timestamp(i32::max_value() as u64)
			.sign_by(&validator(0));
		assert_ne!(default_verify(&header), Err(Error::TimestampOverflow));
	}

	#[test]
	fn verifies_chain_time() {
		// expected import context after verification
		let expect = ImportContext::<AccountId> {
			submitter: None,
			parent_hash: hex!("6e41bff05578fc1db17f6816117969b07d2217f1f9039d8116a82764335991d3").into(),
			parent_header: genesis(),
			parent_total_difficulty: U256::zero(),
			parent_scheduled_change: None,
			validators_set_id: 0,
			validators_set: ValidatorsSet {
				validators: vec![
					hex!("dc5b20847f43d67928f49cd4f85d696b5a7617b5").into(),
					hex!("897df33a7b3c62ade01e22c13d48f98124b4480f").into(),
					hex!("05c987b34c6ef74e0c7e69c6e641120c24164c2d").into(),
				],
				signal_block: None,
				enact_block: HeaderId {
					number: 0,
					hash: hex!("6e41bff05578fc1db17f6816117969b07d2217f1f9039d8116a82764335991d3").into(),
				},
			},
			last_signal_block: None,
		};

		// header is behind
		let header = HeaderBuilder::with_parent(&genesis())
			.timestamp(i32::max_value() as u64 / 2 - 100)
			.sign_by(&validator(1));
		assert_eq!(default_verify(&header).unwrap(), expect);

		// header is ahead
		let header = HeaderBuilder::with_parent(&genesis())
			.timestamp(i32::max_value() as u64 / 2 + 100)
			.sign_by(&validator(1));
		assert_eq!(default_verify(&header), Err(Error::HeaderTimestampIsAhead));

		// header has same timestamp as ConstChainTime
		let header = HeaderBuilder::with_parent(&genesis())
			.timestamp(i32::max_value() as u64 / 2)
			.sign_by(&validator(1));
		assert_eq!(default_verify(&header).unwrap(), expect);
	}

	#[test]
	fn verifies_parent_existence() {
		// when there's no parent in the storage
		let header = HeaderBuilder::with_number(1).sign_by(&validator(0));
		assert_eq!(default_verify(&header), Err(Error::MissingParentBlock));

		// when parent is in the storage
		let header = HeaderBuilder::with_parent(&genesis()).sign_by(&validator(0));
		assert_ne!(default_verify(&header), Err(Error::MissingParentBlock));
	}

	#[test]
	fn verifies_step() {
		// when step is missing from seals
		let mut header = CliqueHeader {
			seal: vec![vec![], vec![]],
			gas_limit: test_clique_variant_config().min_gas_limit,
			parent_hash: genesis().compute_hash(),
			..Default::default()
		};
		assert_eq!(default_verify(&header), Err(Error::MissingStep));

		// when step is the same as for the parent block
		header.seal[0] = rlp_encode(&42u64).to_vec();
		assert_eq!(default_verify(&header), Err(Error::DoubleVote));

		// when step is OK
		header.seal[0] = rlp_encode(&43u64).to_vec();
		assert_ne!(default_verify(&header), Err(Error::DoubleVote));

		// now check with validate_step check enabled
		let mut config = test_clique_variant_config();
		config.validate_step_transition = 0;

		// when step is lesser that for the parent block
		header.seal[0] = rlp_encode(&40u64).to_vec();
		header.seal = vec![vec![40], vec![]];
		assert_eq!(verify_with_config(&config, &header), Err(Error::DoubleVote));

		// when step is OK
		header.seal[0] = rlp_encode(&44u64).to_vec();
		assert_ne!(verify_with_config(&config, &header), Err(Error::DoubleVote));
	}

	#[test]
	fn verifies_empty_step() {
		let mut config = test_clique_variant_config();
		config.empty_steps_transition = 0;

		// when empty step duplicates parent step
		let header = HeaderBuilder::with_parent(&genesis())
			.empty_steps(&[(&validator(0), GENESIS_STEP)])
			.step(GENESIS_STEP + 3)
			.sign_by(&validator(3));
		assert_eq!(verify_with_config(&config, &header), Err(Error::InsufficientProof));

		// when empty step signature check fails
		let header = HeaderBuilder::with_parent(&genesis())
			.empty_steps(&[(&validator(100), GENESIS_STEP + 1)])
			.step(GENESIS_STEP + 3)
			.sign_by(&validator(3));
		assert_eq!(verify_with_config(&config, &header), Err(Error::InsufficientProof));

		// when we are accepting strict empty steps and they come not in order
		config.strict_empty_steps_transition = 0;
		let header = HeaderBuilder::with_parent(&genesis())
			.empty_steps(&[(&validator(2), GENESIS_STEP + 2), (&validator(1), GENESIS_STEP + 1)])
			.step(GENESIS_STEP + 3)
			.sign_by(&validator(3));
		assert_eq!(verify_with_config(&config, &header), Err(Error::InsufficientProof));

		// when empty steps are OK
		let header = HeaderBuilder::with_parent(&genesis())
			.empty_steps(&[(&validator(1), GENESIS_STEP + 1), (&validator(2), GENESIS_STEP + 2)])
			.step(GENESIS_STEP + 3)
			.sign_by(&validator(3));
		assert_ne!(verify_with_config(&config, &header), Err(Error::InsufficientProof));
	}

	#[test]
	fn verifies_chain_score() {
		let mut config = test_clique_variant_config();
		config.validate_score_transition = 0;

		// when chain score is invalid
		let header = HeaderBuilder::with_parent(&genesis())
			.difficulty(100.into())
			.sign_by(&validator(0));
		assert_eq!(verify_with_config(&config, &header), Err(Error::InvalidDifficulty));

		// when chain score is accepted
		let header = HeaderBuilder::with_parent(&genesis()).sign_by(&validator(0));
		assert_ne!(verify_with_config(&config, &header), Err(Error::InvalidDifficulty));
	}

	#[test]
	fn verifies_validator() {
		let good_header = HeaderBuilder::with_parent(&genesis()).sign_by(&validator(1));

		// when header author is invalid
		let mut header = good_header.clone();
		header.author = Default::default();
		assert_eq!(default_verify(&header), Err(Error::NotValidator));

		// when header signature is invalid
		let mut header = good_header.clone();
		header.seal[1] = rlp_encode(&H520::default()).to_vec();
		assert_eq!(default_verify(&header), Err(Error::NotValidator));

		// when everything is OK
		assert_eq!(default_verify(&good_header).map(|_| ()), Ok(()));
	}

	#[test]
	fn pool_verifies_known_blocks() {
		// when header is known
		assert_eq!(
			default_accept_into_pool(|validators| (HeaderBuilder::with_parent_number(2).sign_by_set(validators), None)),
			Err(Error::KnownHeader),
		);
	}

	#[test]
	fn pool_verifies_ancient_blocks() {
		// when header number is less than finalized
		assert_eq!(
			default_accept_into_pool(|validators| (
				HeaderBuilder::with_parent_number(1)
					.gas_limit((GAS_LIMIT + 1).into())
					.sign_by_set(validators),
				None,
			),),
			Err(Error::AncientHeader),
		);
	}

	#[test]
	fn pool_rejects_headers_without_required_receipts() {
		assert_eq!(
			default_accept_into_pool(|_| (
				CliqueHeader {
					number: 20_000_000,
					seal: vec![vec![], vec![]],
					gas_limit: test_clique_variant_config().min_gas_limit,
					log_bloom: (&[0xff; 256]).into(),
					..Default::default()
				},
				None,
			),),
			Err(Error::MissingTransactionsReceipts),
		);
	}

	#[test]
	fn pool_rejects_headers_with_redundant_receipts() {
		assert_eq!(
			default_accept_into_pool(|validators| (
				HeaderBuilder::with_parent_number(3).sign_by_set(validators),
				Some(vec![Receipt {
					gas_used: 1.into(),
					log_bloom: (&[0xff; 256]).into(),
					logs: vec![],
					outcome: TransactionOutcome::Unknown,
				}]),
			),),
			Err(Error::RedundantTransactionsReceipts),
		);
	}

	#[test]
	fn pool_verifies_future_block_number() {
		// when header is too far from the future
		assert_eq!(
			default_accept_into_pool(|validators| (HeaderBuilder::with_number(100).sign_by_set(&validators), None),),
			Err(Error::UnsignedTooFarInTheFuture),
		);
	}

	#[test]
	fn pool_performs_full_verification_when_parent_is_known() {
		// if parent is known, then we'll execute contextual_checks, which
		// checks for DoubleVote
		assert_eq!(
			default_accept_into_pool(|validators| (
				HeaderBuilder::with_parent_number(3)
					.step(GENESIS_STEP + 3)
					.sign_by_set(&validators),
				None,
			),),
			Err(Error::DoubleVote),
		);
	}

	#[test]
	fn pool_performs_validators_checks_when_parent_is_unknown() {
		// if parent is unknown, then we still need to check if header has required signature
		// (even if header will be considered invalid/duplicate later, we can use this signature
		// as a proof of malicious action by this validator)
		assert_eq!(
			default_accept_into_pool(|_| (HeaderBuilder::with_number(8).step(8).sign_by(&validator(1)), None,)),
			Err(Error::NotValidator),
		);
	}

	#[test]
	fn pool_verifies_header_with_known_parent() {
		let mut hash = None;
		assert_eq!(
			default_accept_into_pool(|validators| {
				let header = HeaderBuilder::with_parent_number(3).sign_by_set(validators);
				hash = Some(header.compute_hash());
				(header, None)
			}),
			Ok((
				// no tags are required
				vec![],
				// header provides two tags
				vec![
					(4u64, validators_addresses(3)[1]).encode(),
					(4u64, hash.unwrap()).encode(),
				],
			)),
		);
	}

	#[test]
	fn pool_verifies_header_with_unknown_parent() {
		let mut id = None;
		let mut parent_id = None;
		assert_eq!(
			default_accept_into_pool(|validators| {
				let header = HeaderBuilder::with_number(5)
					.step(GENESIS_STEP + 5)
					.sign_by_set(validators);
				id = Some(header.compute_id());
				parent_id = header.parent_id();
				(header, None)
			}),
			Ok((
				// parent tag required
				vec![parent_id.unwrap().encode()],
				// header provides two tags
				vec![(5u64, validator_address(2)).encode(), id.unwrap().encode(),],
			)),
		);
	}

	#[test]
	fn pool_uses_next_validators_set_when_finalized_fails() {
		assert_eq!(
			default_accept_into_pool(|actual_validators| {
				// change finalized set at parent header
				change_validators_set_at(3, validators_addresses(1), None);

				// header is signed using wrong set
				let header = HeaderBuilder::with_number(5)
					.step(GENESIS_STEP + 2)
					.sign_by_set(actual_validators);

				(header, None)
			}),
			Err(Error::NotValidator),
		);

		let mut id = None;
		let mut parent_id = None;
		assert_eq!(
			default_accept_into_pool(|actual_validators| {
				// change finalized set at parent header + signal valid set at parent block
				change_validators_set_at(3, validators_addresses(10), Some(validators_addresses(3)));

				// header is signed using wrong set
				let header = HeaderBuilder::with_number(5)
					.step(GENESIS_STEP + 2)
					.sign_by_set(actual_validators);
				id = Some(header.compute_id());
				parent_id = header.parent_id();

				(header, None)
			}),
			Ok((
				// parent tag required
				vec![parent_id.unwrap().encode(),],
				// header provides two tags
				vec![(5u64, validator_address(2)).encode(), id.unwrap().encode(),],
			)),
		);
	}

	#[test]
	fn pool_rejects_headers_with_invalid_receipts() {
		assert_eq!(
			default_accept_into_pool(|validators| {
				let header = HeaderBuilder::with_parent_number(3)
					.log_bloom((&[0xff; 256]).into())
					.sign_by_set(validators);
				(header, Some(vec![validators_change_receipt(Default::default())]))
			}),
			Err(Error::TransactionsReceiptsMismatch),
		);
	}

	#[test]
	fn pool_accepts_headers_with_valid_receipts() {
		let mut hash = None;
		let receipts = vec![validators_change_receipt(Default::default())];
		let receipts_root = compute_merkle_root(receipts.iter().map(|r| r.rlp()));

		assert_eq!(
			default_accept_into_pool(|validators| {
				let header = HeaderBuilder::with_parent_number(3)
					.log_bloom((&[0xff; 256]).into())
					.receipts_root(receipts_root)
					.sign_by_set(validators);
				hash = Some(header.compute_hash());
				(header, Some(receipts.clone()))
			}),
			Ok((
				// no tags are required
				vec![],
				// header provides two tags
				vec![
					(4u64, validators_addresses(3)[1]).encode(),
					(4u64, hash.unwrap()).encode(),
				],
			)),
		);
	}
}