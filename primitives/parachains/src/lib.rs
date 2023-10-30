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

//! Primitives of parachains module.

#![cfg_attr(not(feature = "std"), no_std)]

// crates.io
use codec::{Decode, Encode, MaxEncodedLen};
use scale_info::TypeInfo;
// darwinia-network
use bp_polkadot_core::{
	parachains::{ParaHash, ParaHead, ParaId},
	BlockNumber as RelayBlockNumber,
};
use bp_runtime::{StorageDoubleMapKeyProvider, StorageMapKeyProvider};
// substrate
use frame_support::{Blake2_128Concat, Twox64Concat};
use sp_core::storage::StorageKey;
use sp_runtime::RuntimeDebug;

/// Best known parachain head hash.
#[derive(Clone, PartialEq, Decode, Encode, MaxEncodedLen, RuntimeDebug, TypeInfo)]
pub struct BestParaHeadHash {
	/// Number of relay block where this head has been read.
	///
	/// Parachain head is opaque to relay chain. So we can't simply decode it as a header of
	/// parachains and call `block_number()` on it. Instead, we're using the fact that parachain
	/// head is always built on top of previous head (because it is blockchain) and relay chain
	/// always imports parachain heads in order. What it means for us is that at any given
	/// **finalized** relay block `B`, head of parachain will be ancestor (or the same) of all
	/// parachain heads available at descendants of `B`.
	pub at_relay_block_number: RelayBlockNumber,
	/// Hash of parachain head.
	pub head_hash: ParaHash,
}

/// Best known parachain head as it is stored in the runtime storage.
#[derive(PartialEq, Decode, Encode, MaxEncodedLen, RuntimeDebug, TypeInfo)]
pub struct ParaInfo {
	/// Best known parachain head hash.
	pub best_head_hash: BestParaHeadHash,
	/// Current ring buffer position for this parachain.
	pub next_imported_hash_position: u32,
}

/// Can be use to access the runtime storage key of the parachains info at the target chain.
///
/// The info is stored by the `pallet-bridge-parachains` pallet in the `ParasInfo` map.
pub struct ParasInfoKeyProvider;
impl StorageMapKeyProvider for ParasInfoKeyProvider {
	type Hasher = Blake2_128Concat;
	type Key = ParaId;
	type Value = ParaInfo;

	const MAP_NAME: &'static str = "ParasInfo";
}

/// Can be use to access the runtime storage key of the parachain head at the target chain.
///
/// The head is stored by the `pallet-bridge-parachains` pallet in the `ImportedParaHeads` map.
pub struct ImportedParaHeadsKeyProvider;
impl StorageDoubleMapKeyProvider for ImportedParaHeadsKeyProvider {
	type Hasher1 = Blake2_128Concat;
	type Hasher2 = Blake2_128Concat;
	type Key1 = ParaId;
	type Key2 = ParaHash;
	type Value = ParaHead;

	const MAP_NAME: &'static str = "ImportedParaHeads";
}

/// Returns runtime storage key of given parachain head at the source chain.
///
/// The head is stored by the `paras` pallet in the `Heads` map.
pub fn parachain_head_storage_key_at_source(
	paras_pallet_name: &str,
	para_id: ParaId,
) -> StorageKey {
	bp_runtime::storage_map_final_key::<Twox64Concat>(paras_pallet_name, "Heads", &para_id.encode())
}
