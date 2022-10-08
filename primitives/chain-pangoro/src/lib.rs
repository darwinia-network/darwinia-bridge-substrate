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

	pub const EXISTENTIAL_DEPOSIT: Balance = 0;

	pub const SESSION_LENGTH: BlockNumber = 2 * HOURS;
}
pub use copy_paste_from_darwinia::*;

pub use bp_darwinia_core::*;

// --- paritytech ---
use bp_messages::{LaneId, MessageDetails, MessageNonce};
use frame_support::Parameter;
use sp_std::prelude::*;

/// Pangoro Chain.
pub type Pangoro = DarwiniaLike;

/// Name of the With-Pangoro GRANDPA pallet instance that is deployed at bridged chains.
pub const WITH_PANGORO_GRANDPA_PALLET_NAME: &str = "BridgePangoroGrandpa";
/// Name of the With-Pangoro messages pallet instance that is deployed at bridged chains.
pub const WITH_PANGORO_MESSAGES_PALLET_NAME: &str = "BridgePangoroMessages";

/// Name of the `PangoroFinalityApi::best_finalized` runtime method.
pub const BEST_FINALIZED_PANGORO_HEADER_METHOD: &str = "PangoroFinalityApi_best_finalized";

/// Name of the `ToPangoroOutboundLaneApi::message_details` runtime method.
pub const TO_PANGORO_MESSAGE_DETAILS_METHOD: &str = "ToPangoroOutboundLaneApi_message_details";

sp_api::decl_runtime_apis! {
	/// API for querying information about the finalized Pangoro headers.
	///
	/// This API is implemented by runtimes that are bridging with the Pangoro chain, not the
	/// Pangoro runtime itself.
	pub trait PangoroFinalityApi {
		/// Returns number and hash of the best finalized header known to the bridge module.
		fn best_finalized() -> (BlockNumber, Hash);
	}

	/// Outbound message lane API for messages that are sent to Pangoro chain.
	///
	/// This API is implemented by runtimes that are sending messages to Pangoro chain, not the
	/// Pangoro runtime itself.
	pub trait ToPangoroOutboundLaneApi<OutboundMessageFee: Parameter, OutboundPayload: Parameter> {
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
	}
}
