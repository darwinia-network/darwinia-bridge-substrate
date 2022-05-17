// This file is part of Darwinia.
//
// Copyright (C) 2018-2022 Darwinia Network
// SPDX-License-Identifier: GPL-3.0
//
// Darwinia is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// Darwinia is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with Darwinia. If not, see <https://www.gnu.org/licenses/>.

#![cfg_attr(not(feature = "std"), no_std)]

mod copy_paste_from_darwinia {
	// --- darwinia-network ---
	use bp_darwinia_core::*;
	// --- paritytech ---
	use sp_version::RuntimeVersion;

	pub const VERSION: RuntimeVersion = RuntimeVersion {
		spec_name: sp_runtime::create_runtime_str!("Pangolin Parachain"),
		impl_name: sp_runtime::create_runtime_str!("Pangolin Parachain"),
		authoring_version: 1,
		spec_version: 3,
		impl_version: 1,
		apis: sp_version::create_apis_vec![[]],
		transaction_version: 1,
	};

	pub const EXISTENTIAL_DEPOSIT: Balance = 0;
}
pub use copy_paste_from_darwinia::*;

pub use bp_darwinia_core::*;

// --- paritytech ---
use bp_messages::{LaneId, MessageDetails, MessageNonce, UnrewardedRelayersState};
use frame_support::Parameter;
use sp_std::prelude::*;

/// PangolinParachain Chain.
pub type PangolinParachain = DarwiniaLike;

/// Name of the With-PangolinParachain GRANDPA pallet instance that is deployed at bridged chains.
pub const WITH_PANGOLIN_PARACHAIN_GRANDPA_PALLET_NAME: &str = "BridgePangolinParachainGrandpa";
/// Name of the With-PangolinParachain messages pallet instance that is deployed at bridged chains.
pub const WITH_PANGOLIN_PARACHAIN_MESSAGES_PALLET_NAME: &str = "BridgePangolinParachainMessages";

/// Name of the `PangolinParachainFinalityApi::best_finalized` runtime method.
pub const BEST_FINALIZED_PANGOLIN_PARACHAIN_HEADER_METHOD: &str =
	"PangolinParachainFinalityApi_best_finalized";

/// Name of the `ToPangolinParachainOutboundLaneApi::message_details` runtime method.
pub const TO_PANGOLIN_PARACHAIN_MESSAGE_DETAILS_METHOD: &str =
	"ToPangolinParachainOutboundLaneApi_message_details";
/// Name of the `ToPangolinParachainOutboundLaneApi::latest_received_nonce` runtime method.
pub const TO_PANGOLIN_PARACHAIN_LATEST_RECEIVED_NONCE_METHOD: &str =
	"ToPangolinParachainOutboundLaneApi_latest_received_nonce";
/// Name of the `ToPangolinParachainOutboundLaneApi::latest_generated_nonce` runtime method.
pub const TO_PANGOLIN_PARACHAIN_LATEST_GENERATED_NONCE_METHOD: &str =
	"ToPangolinParachainOutboundLaneApi_latest_generated_nonce";

/// Name of the `FromPangolinParachainInboundLaneApi::latest_received_nonce` runtime method.
pub const FROM_PANGOLIN_PARACHAIN_LATEST_RECEIVED_NONCE_METHOD: &str =
	"FromPangolinParachainInboundLaneApi_latest_received_nonce";
/// Name of the `FromPangolinParachainInboundLaneApi::latest_onfirmed_nonce` runtime method.
pub const FROM_PANGOLIN_PARACHAIN_LATEST_CONFIRMED_NONCE_METHOD: &str =
	"FromPangolinParachainInboundLaneApi_latest_confirmed_nonce";
/// Name of the `FromPangolinParachainInboundLaneApi::unrewarded_relayers_state` runtime method.
pub const FROM_PANGOLIN_PARACHAIN_UNREWARDED_RELAYERS_STATE: &str =
	"FromPangolinParachainInboundLaneApi_unrewarded_relayers_state";

sp_api::decl_runtime_apis! {
	/// API for querying information about the finalized PangolinParachain headers.
	///
	/// This API is implemented by runtimes that are bridging with the PangolinParachain chain, not the
	/// PangolinParachain runtime itself.
	pub trait PangolinParachainFinalityApi {
		/// Returns number and hash of the best finalized header known to the bridge module.
		fn best_finalized() -> (BlockNumber, Hash);
	}

	/// Outbound message lane API for messages that are sent to PangolinParachain chain.
	///
	/// This API is implemented by runtimes that are sending messages to PangolinParachain chain, not the
	/// PangolinParachain runtime itself.
	pub trait ToPangolinParachainOutboundLaneApi<OutboundMessageFee: Parameter, OutboundPayload: Parameter> {
		/// Returns dispatch weight, encoded payload size and delivery+dispatch fee of all
		/// messages in given inclusive range.
		///
		/// If some (or all) messages are missing from the storage, they'll also will
		/// be missing from the resulting vector. The vector is ordered by the nonce.
		fn message_details(
			lane: LaneId,
			begin: MessageNonce,
			end: MessageNonce,
		) -> Vec<MessageDetails<OutboundMessageFee>>;
		/// Returns nonce of the latest message, received by bridged chain.
		fn latest_received_nonce(lane: LaneId) -> MessageNonce;
		/// Returns nonce of the latest message, generated by given lane.
		fn latest_generated_nonce(lane: LaneId) -> MessageNonce;
	}

	/// Inbound message lane API for messages sent by PangolinParachain chain.
	///
	/// This API is implemented by runtimes that are receiving messages from PangolinParachain chain, not the
	/// PangolinParachain runtime itself.
	pub trait FromPangolinParachainInboundLaneApi {
		/// Returns nonce of the latest message, received by given lane.
		fn latest_received_nonce(lane: LaneId) -> MessageNonce;
		/// Nonce of latest message that has been confirmed to the bridged chain.
		fn latest_confirmed_nonce(lane: LaneId) -> MessageNonce;
		/// State of the unrewarded relayers set at given lane.
		fn unrewarded_relayers_state(lane: LaneId) -> UnrewardedRelayersState;
	}
}