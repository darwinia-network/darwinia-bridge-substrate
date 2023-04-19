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

//! Autogenerated weights for `pallet_bridge_messages`
//!
//! THIS FILE WAS AUTO-GENERATED USING THE SUBSTRATE BENCHMARK CLI VERSION 4.0.0-dev
//! DATE: 2021-12-28, STEPS: 50, REPEAT: 20
//! LOW RANGE: [], HIGH RANGE: []
//! EXECUTION: Some(Wasm), WASM-EXECUTION: Compiled
//! CHAIN: Some("dev"), DB CACHE: 128

// Executed Command:
// target/release/millau-bridge-node
// benchmark
// --chain=dev
// --steps=50
// --repeat=20
// --pallet=pallet_bridge_messages
// --extrinsic=*
// --execution=wasm
// --wasm-execution=Compiled
// --heap-pages=4096
// --output=./modules/messages/src/weights.rs
// --template=./.maintain/millau-weight-template.hbs

#![allow(clippy::all)]
#![allow(unused_parens)]
#![allow(unused_imports)]

use frame_support::{
	traits::Get,
	weights::{constants::RocksDbWeight, Weight},
};
use sp_std::marker::PhantomData;

/// Weight functions needed for `pallet_bridge_messages`.
pub trait WeightInfo {
	fn receive_single_message_proof() -> Weight;
	fn receive_two_messages_proof() -> Weight;
	fn receive_single_message_proof_with_outbound_lane_state() -> Weight;
	fn receive_single_message_proof_1_kb() -> Weight;
	fn receive_single_message_proof_16_kb() -> Weight;
	fn receive_delivery_proof_for_single_message() -> Weight;
	fn receive_delivery_proof_for_two_messages_by_single_relayer() -> Weight;
	fn receive_delivery_proof_for_two_messages_by_two_relayers() -> Weight;
}

/// Weights for `pallet_bridge_messages` using the Millau node and recommended hardware.
pub struct BridgeWeight<T>(PhantomData<T>);
impl<T: frame_system::Config> WeightInfo for BridgeWeight<T> {
	fn receive_single_message_proof() -> Weight {
		Weight::from_ref_time(179_892_000 as u64)
			.saturating_add(T::DbWeight::get().reads(6 as u64))
			.saturating_add(T::DbWeight::get().writes(3 as u64))
	}

	fn receive_two_messages_proof() -> Weight {
		Weight::from_ref_time(291_793_000 as u64)
			.saturating_add(T::DbWeight::get().reads(6 as u64))
			.saturating_add(T::DbWeight::get().writes(3 as u64))
	}

	fn receive_single_message_proof_with_outbound_lane_state() -> Weight {
		Weight::from_ref_time(192_191_000 as u64)
			.saturating_add(T::DbWeight::get().reads(6 as u64))
			.saturating_add(T::DbWeight::get().writes(3 as u64))
	}

	fn receive_single_message_proof_1_kb() -> Weight {
		Weight::from_ref_time(202_104_000 as u64)
			.saturating_add(T::DbWeight::get().reads(6 as u64))
			.saturating_add(T::DbWeight::get().writes(3 as u64))
	}

	fn receive_single_message_proof_16_kb() -> Weight {
		Weight::from_ref_time(357_144_000 as u64)
			.saturating_add(T::DbWeight::get().reads(6 as u64))
			.saturating_add(T::DbWeight::get().writes(3 as u64))
	}

	fn receive_delivery_proof_for_single_message() -> Weight {
		Weight::from_ref_time(107_631_000 as u64)
			.saturating_add(T::DbWeight::get().reads(6 as u64))
			.saturating_add(T::DbWeight::get().writes(3 as u64))
	}

	fn receive_delivery_proof_for_two_messages_by_single_relayer() -> Weight {
		Weight::from_ref_time(113_885_000 as u64)
			.saturating_add(T::DbWeight::get().reads(7 as u64))
			.saturating_add(T::DbWeight::get().writes(3 as u64))
	}

	fn receive_delivery_proof_for_two_messages_by_two_relayers() -> Weight {
		Weight::from_ref_time(155_151_000 as u64)
			.saturating_add(T::DbWeight::get().reads(8 as u64))
			.saturating_add(T::DbWeight::get().writes(4 as u64))
	}
}

// For backwards compatibility and tests
impl WeightInfo for () {
	fn receive_single_message_proof() -> Weight {
		Weight::from_ref_time(179_892_000 as u64)
			.saturating_add(RocksDbWeight::get().reads(6 as u64))
			.saturating_add(RocksDbWeight::get().writes(3 as u64))
	}

	fn receive_two_messages_proof() -> Weight {
		Weight::from_ref_time(291_793_000 as u64)
			.saturating_add(RocksDbWeight::get().reads(6 as u64))
			.saturating_add(RocksDbWeight::get().writes(3 as u64))
	}

	fn receive_single_message_proof_with_outbound_lane_state() -> Weight {
		Weight::from_ref_time(192_191_000 as u64)
			.saturating_add(RocksDbWeight::get().reads(6 as u64))
			.saturating_add(RocksDbWeight::get().writes(3 as u64))
	}

	fn receive_single_message_proof_1_kb() -> Weight {
		Weight::from_ref_time(202_104_000 as u64)
			.saturating_add(RocksDbWeight::get().reads(6 as u64))
			.saturating_add(RocksDbWeight::get().writes(3 as u64))
	}

	fn receive_single_message_proof_16_kb() -> Weight {
		Weight::from_ref_time(357_144_000 as u64)
			.saturating_add(RocksDbWeight::get().reads(6 as u64))
			.saturating_add(RocksDbWeight::get().writes(3 as u64))
	}

	fn receive_delivery_proof_for_single_message() -> Weight {
		Weight::from_ref_time(107_631_000 as u64)
			.saturating_add(RocksDbWeight::get().reads(6 as u64))
			.saturating_add(RocksDbWeight::get().writes(3 as u64))
	}

	fn receive_delivery_proof_for_two_messages_by_single_relayer() -> Weight {
		Weight::from_ref_time(113_885_000 as u64)
			.saturating_add(RocksDbWeight::get().reads(7 as u64))
			.saturating_add(RocksDbWeight::get().writes(3 as u64))
	}

	fn receive_delivery_proof_for_two_messages_by_two_relayers() -> Weight {
		Weight::from_ref_time(155_151_000 as u64)
			.saturating_add(RocksDbWeight::get().reads(8 as u64))
			.saturating_add(RocksDbWeight::get().writes(4 as u64))
	}
}
