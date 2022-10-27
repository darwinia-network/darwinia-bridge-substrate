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

//! Types that allow runtime to act as a source/target endpoint of message lanes.
//!
//! Messages are assumed to be encoded `Call`s of the target chain. Call-dispatch
//! pallet is used to dispatch incoming messages. Message identified by a tuple
//! of to elements - message lane id and message nonce.

// core
use core::marker::PhantomData;
// crates.io
use codec::{Decode, DecodeLimit, Encode, MaxEncodedLen};
use hash_db::Hasher;
use scale_info::TypeInfo;
// darwinia-network
use bp_messages::{
	source_chain::LaneMessageVerifier,
	target_chain::{DispatchMessage, MessageDispatch, ProvedLaneMessages, ProvedMessages},
	InboundLaneData, LaneId, Message, MessageData, MessageKey, MessageNonce, OutboundLaneData,
};
use bp_polkadot_core::parachains::{ParaHash, ParaHasher, ParaId};
use bp_runtime::{messages::MessageDispatchResult, ChainId, Size, StorageProofChecker};
// paritytech
use frame_support::{
	traits::{Currency, Get},
	weights::Weight,
	RuntimeDebug,
};
use sp_runtime::{
	traits::{CheckedAdd, CheckedDiv, CheckedMul, Header as HeaderT},
	FixedPointOperand,
};
use sp_std::prelude::*;
use sp_trie::StorageProof;

/// Bidirectional message bridge.
pub trait MessageBridge {
	/// Relayer interest (in percents).
	const RELAYER_FEE_PERCENT: u32;

	/// Identifier of this chain.
	const THIS_CHAIN_ID: ChainId;
	/// Identifier of the Bridged chain.
	const BRIDGED_CHAIN_ID: ChainId;
	/// Name of the paired messages pallet instance at the Bridged chain.
	///
	/// Should be the name that is used in the `construct_runtime!()` macro.
	const BRIDGED_MESSAGES_PALLET_NAME: &'static str;

	/// This chain in context of message bridge.
	type ThisChain: ThisChainWithMessages;
	/// Bridged chain in context of message bridge.
	type BridgedChain: BridgedChainWithMessages;
}

/// Chain that has `pallet-bridge-messages` and `dispatch` modules.
pub trait ChainWithMessages {
	/// Hash used in the chain.
	type Hash: Decode;
	/// Accound id on the chain.
	type AccountId: Encode + Decode + MaxEncodedLen;
	/// Public key of the chain account that may be used to verify signatures.
	type Signer: Encode + Decode;
	/// Signature type used on the chain.
	type Signature: Encode + Decode;
	/// Type of weight that is used on the chain. This would almost always be a regular
	/// `frame_support::weight::Weight`. But since the meaning of weight on different chains
	/// may be different, the `WeightOf<>` construct is used to avoid confusion between
	/// different weights.
	type Weight: From<frame_support::weights::Weight> + PartialOrd;
	/// Type of balances that is used on the chain.
	type Balance: Encode
		+ Decode
		+ CheckedAdd
		+ CheckedDiv
		+ CheckedMul
		+ PartialOrd
		+ From<u32>
		+ Copy;
}

/// This chain that has `pallet-bridge-messages` and `dispatch` modules.
pub trait ThisChainWithMessages: ChainWithMessages {
	/// Call origin on the chain.
	type Origin;
	/// Call type on the chain.
	type Call: Encode + Decode;

	/// Do we accept message sent by given origin to given lane?
	fn is_message_accepted(origin: &Self::Origin, lane: &LaneId) -> bool;

	/// Maximal number of pending (not yet delivered) messages at This chain.
	///
	/// Any messages over this limit, will be rejected.
	fn maximal_pending_messages_at_outbound_lane() -> MessageNonce;
}

/// Bridged chain that has `pallet-bridge-messages` and `dispatch` modules.
pub trait BridgedChainWithMessages: ChainWithMessages {
	/// Maximal extrinsic size at Bridged chain.
	fn maximal_extrinsic_size() -> u32;

	/// Returns `true` if message dispatch weight is withing expected limits. `false` means
	/// that the message is too heavy to be sent over the bridge and shall be rejected.
	fn verify_dispatch_weight(message_payload: &[u8], payload_weight: &Weight) -> bool;
}

/// This chain in context of message bridge.
pub type ThisChain<B> = <B as MessageBridge>::ThisChain;
/// Bridged chain in context of message bridge.
pub type BridgedChain<B> = <B as MessageBridge>::BridgedChain;
/// Hash used on the chain.
pub type HashOf<C> = <C as ChainWithMessages>::Hash;
/// Account id used on the chain.
pub type AccountIdOf<C> = <C as ChainWithMessages>::AccountId;
/// Public key of the chain account that may be used to verify signature.
pub type SignerOf<C> = <C as ChainWithMessages>::Signer;
/// Signature type used on the chain.
pub type SignatureOf<C> = <C as ChainWithMessages>::Signature;
/// Type of weight that used on the chain.
pub type WeightOf<C> = <C as ChainWithMessages>::Weight;
/// Type of balances that is used on the chain.
pub type BalanceOf<C> = <C as ChainWithMessages>::Balance;
/// Type of origin that is used on the chain.
pub type OriginOf<C> = <C as ThisChainWithMessages>::Origin;
/// Type of call that is used on this chain.
pub type CallOf<C> = <C as ThisChainWithMessages>::Call;

/// Raw storage proof type (just raw trie nodes).
pub type RawStorageProof = Vec<Vec<u8>>;

/// Sub-module that is declaring types required for processing This -> Bridged chain messages.
pub mod source {
	use super::*;

	/// Encoded Call of the Bridged chain. We never try to decode it on This chain.
	pub type BridgedChainOpaqueCall = Vec<u8>;

	pub type FromThisChainMessagePayload = Vec<u8>;

	/// Messages delivery proof from bridged chain:
	///
	/// - hash of finalized header;
	/// - storage proof of inbound lane state;
	/// - lane id.
	#[derive(Clone, Encode, Decode, Eq, PartialEq, RuntimeDebug, TypeInfo)]
	pub struct FromBridgedChainMessagesDeliveryProof<BridgedHeaderHash> {
		/// Hash of the bridge header the proof is for.
		pub bridged_header_hash: BridgedHeaderHash,
		/// Storage trie proof generated for [`Self::bridged_header_hash`].
		pub storage_proof: RawStorageProof,
		/// Lane id of which messages were delivered and the proof is for.
		pub lane: LaneId,
	}

	impl<BridgedHeaderHash> Size for FromBridgedChainMessagesDeliveryProof<BridgedHeaderHash> {
		fn size(&self) -> u32 {
			u32::try_from(
				self.storage_proof.iter().fold(0usize, |sum, node| sum.saturating_add(node.len())),
			)
			.unwrap_or(u32::MAX)
		}
	}

	/// 'Parsed' message delivery proof - inbound lane id and its state.
	pub type ParsedMessagesDeliveryProofFromBridgedChain<B> =
		(LaneId, InboundLaneData<AccountIdOf<ThisChain<B>>>);

	/// Message verifier that is doing all basic checks.
	///
	/// This verifier assumes following:
	///
	/// - all message lanes are equivalent, so all checks are the same;
	///
	/// Following checks are made:
	///
	/// - message is rejected if its lane is currently blocked;
	/// - message is rejected if there are too many pending (undelivered) messages at the outbound
	///   lane;
	/// - check that the sender has rights to dispatch the call on target chain using provided
	///   dispatch origin;
	/// - check that the sender has paid enough funds for both message delivery and dispatch.
	#[derive(RuntimeDebug)]
	pub struct FromThisChainMessageVerifier<B, F, I>(PhantomData<(B, F, I)>);

	/// The error message returned from LaneMessageVerifier when outbound lane is disabled.
	pub const MESSAGE_REJECTED_BY_OUTBOUND_LANE: &str =
		"The outbound message lane has rejected the message.";
	/// The error message returned from LaneMessageVerifier when too many pending messages at the
	/// lane.
	pub const TOO_MANY_PENDING_MESSAGES: &str = "Too many pending messages at the lane.";
	/// The error message returned from LaneMessageVerifier when call origin is mismatch.
	pub const BAD_ORIGIN: &str = "Unable to match the source origin to expected target origin.";
	/// The error message returned from LaneMessageVerifier when the message fee is too low.
	pub const TOO_LOW_FEE: &str = "Provided fee is below minimal threshold required by the lane.";

	impl<B, F, I>
		LaneMessageVerifier<
			OriginOf<ThisChain<B>>,
			AccountIdOf<ThisChain<B>>,
			FromThisChainMessagePayload,
			BalanceOf<ThisChain<B>>,
		> for FromThisChainMessageVerifier<B, F, I>
	where
		B: MessageBridge,
		F: pallet_fee_market::Config<I>,
		I: 'static,
		// matches requirements from the `frame_system::Config::Origin`
		OriginOf<ThisChain<B>>: Clone
			+ Into<Result<frame_system::RawOrigin<AccountIdOf<ThisChain<B>>>, OriginOf<ThisChain<B>>>>,
		AccountIdOf<ThisChain<B>>: PartialEq + Clone,
		pallet_fee_market::BalanceOf<F, I>: From<BalanceOf<ThisChain<B>>>,
	{
		type Error = &'static str;

		#[allow(clippy::single_match)]
		#[cfg(not(feature = "runtime-benchmarks"))]
		fn verify_message(
			submitter: &OriginOf<ThisChain<B>>,
			delivery_and_dispatch_fee: &BalanceOf<ThisChain<B>>,
			lane: &LaneId,
			lane_outbound_data: &OutboundLaneData,
			_payload: &FromThisChainMessagePayload,
		) -> Result<(), Self::Error> {
			// reject message if lane is blocked
			if !ThisChain::<B>::is_message_accepted(submitter, lane) {
				return Err(MESSAGE_REJECTED_BY_OUTBOUND_LANE);
			}

			// reject message if there are too many pending messages at this lane
			let max_pending_messages = ThisChain::<B>::maximal_pending_messages_at_outbound_lane();
			let pending_messages = lane_outbound_data
				.latest_generated_nonce
				.saturating_sub(lane_outbound_data.latest_received_nonce);
			if pending_messages > max_pending_messages {
				return Err(TOO_MANY_PENDING_MESSAGES);
			}

			// Do the dispatch-specific check. We assume that the target chain uses
			// `Dispatch`, so we verify the message accordingly.
			let raw_origin_or_err: Result<
				frame_system::RawOrigin<AccountIdOf<ThisChain<B>>>,
				OriginOf<ThisChain<B>>,
			> = submitter.clone().into();
			if let Ok(_raw_origin) = raw_origin_or_err {
				// pallet_bridge_dispatch::verify_message_origin(&raw_origin, payload)
				// 	.map(drop)
				// 	.map_err(|_| BAD_ORIGIN)?;
			} else {
				// so what it means that we've failed to convert origin to the
				// `frame_system::RawOrigin`? now it means that the custom pallet origin has
				// been used to send the message. Do we need to verify it? The answer is no,
				// because pallet may craft any origin (e.g. root) && we can't verify whether it
				// is valid, or not.
			};

			// Do the delivery_and_dispatch_fee. We assume that the delivery and dispatch fee always
			// greater than the fee market provided fee.
			if let Some(market_fee) = pallet_fee_market::Pallet::<F, I>::market_fee() {
				let message_fee: pallet_fee_market::BalanceOf<F, I> =
					(*delivery_and_dispatch_fee).into();

				// compare with actual fee paid
				if message_fee < market_fee {
					return Err(TOO_LOW_FEE);
				}
			} else {
				const NO_MARKET_FEE: &str = "The fee market are not ready for accepting messages.";

				return Err(NO_MARKET_FEE);
			}

			Ok(())
		}

		#[cfg(feature = "runtime-benchmarks")]
		fn verify_message(
			_submitter: &OriginOf<ThisChain<B>>,
			_delivery_and_dispatch_fee: &BalanceOf<ThisChain<B>>,
			_lane: &LaneId,
			_lane_outbound_data: &OutboundLaneData,
			_payload: &FromThisChainMessagePayload<B>,
		) -> Result<(), Self::Error> {
			Ok(())
		}
	}

	/// Return maximal message size of This -> Bridged chain message.
	pub fn maximal_message_size<B: MessageBridge>() -> u32 {
		super::target::maximal_incoming_message_size(BridgedChain::<B>::maximal_extrinsic_size())
	}

	/// Do basic Bridged-chain specific verification of This -> Bridged chain message.
	///
	/// Ok result from this function means that the delivery transaction with this message
	/// may be 'mined' by the target chain. But the lane may have its own checks (e.g. fee
	/// check) that would reject message (see `FromThisChainMessageVerifier`).
	pub fn verify_chain_message<B: MessageBridge>(
		payload: &FromThisChainMessagePayload,
	) -> Result<(), &'static str> {
		// if !BridgedChain::<B>::verify_dispatch_weight(&payload.call, &payload.weight) {
		// 	return Err("Incorrect message weight declared");
		// }

		// The maximal size of extrinsic at Substrate-based chain depends on the
		// `frame_system::Config::MaximumBlockLength` and
		// `frame_system::Config::AvailableBlockRatio` constants. This check is here to be sure that
		// the lane won't stuck because message is too large to fit into delivery transaction.
		//
		// **IMPORTANT NOTE**: the delivery transaction contains storage proof of the message, not
		// the message itself. The proof is always larger than the message. But unless chain state
		// is enormously large, it should be several dozens/hundreds of bytes. The delivery
		// transaction also contains signatures and signed extensions. Because of this, we reserve
		// 1/3 of the the maximal extrinsic weight for this data.
		if payload.len() > maximal_message_size::<B>() as usize {
			return Err("The message is too large to be sent over the lane");
		}

		Ok(())
	}

	/// Verify proof of This -> Bridged chain messages delivery.
	///
	/// This function is used when Bridged chain is directly using GRANDPA finality. For Bridged
	/// parachains, please use the `verify_messages_delivery_proof_from_parachain`.
	pub fn verify_messages_delivery_proof<B: MessageBridge, ThisRuntime, GrandpaInstance: 'static>(
		proof: FromBridgedChainMessagesDeliveryProof<HashOf<BridgedChain<B>>>,
	) -> Result<ParsedMessagesDeliveryProofFromBridgedChain<B>, &'static str>
	where
		ThisRuntime: pallet_bridge_grandpa::Config<GrandpaInstance>,
		HashOf<BridgedChain<B>>: Into<
			bp_runtime::HashOf<
				<ThisRuntime as pallet_bridge_grandpa::Config<GrandpaInstance>>::BridgedChain,
			>,
		>,
	{
		let FromBridgedChainMessagesDeliveryProof { bridged_header_hash, storage_proof, lane } =
			proof;
		pallet_bridge_grandpa::Pallet::<ThisRuntime, GrandpaInstance>::parse_finalized_storage_proof(
			bridged_header_hash.into(),
			StorageProof::new(storage_proof),
			|storage| do_verify_messages_delivery_proof::<
				B,
				bp_runtime::HasherOf<
					<ThisRuntime as pallet_bridge_grandpa::Config<GrandpaInstance>>::BridgedChain,
				>,
			>(lane, storage),
		)
		.map_err(<&'static str>::from)?
	}

	/// Verify proof of This -> Bridged chain messages delivery.
	///
	/// This function is used when Bridged chain is using parachain finality. For Bridged
	/// chains with direct GRANDPA finality, please use the `verify_messages_delivery_proof`.
	///
	/// This function currently only supports parachains, which are using header type that
	/// implements `sp_runtime::traits::Header` trait.
	pub fn verify_messages_delivery_proof_from_parachain<
		B,
		BridgedHeader,
		ThisRuntime,
		ParachainsInstance: 'static,
	>(
		bridged_parachain: ParaId,
		proof: FromBridgedChainMessagesDeliveryProof<HashOf<BridgedChain<B>>>,
	) -> Result<ParsedMessagesDeliveryProofFromBridgedChain<B>, &'static str>
	where
		B: MessageBridge,
		B::BridgedChain: ChainWithMessages<Hash = ParaHash>,
		BridgedHeader: HeaderT<Hash = HashOf<BridgedChain<B>>>,
		ThisRuntime: pallet_bridge_parachains::Config<ParachainsInstance>,
	{
		let FromBridgedChainMessagesDeliveryProof { bridged_header_hash, storage_proof, lane } =
			proof;
		pallet_bridge_parachains::Pallet::<ThisRuntime, ParachainsInstance>::parse_finalized_storage_proof(
			bridged_parachain,
			bridged_header_hash,
			StorageProof::new(storage_proof),
			|para_head| BridgedHeader::decode(&mut &para_head.0[..]).ok().map(|h| *h.state_root()),
			|storage| do_verify_messages_delivery_proof::<B, ParaHasher>(lane, storage),
		)
		.map_err(<&'static str>::from)?
	}

	/// The essense of This -> Bridged chain messages delivery proof verification.
	fn do_verify_messages_delivery_proof<B: MessageBridge, H: Hasher>(
		lane: LaneId,
		storage: bp_runtime::StorageProofChecker<H>,
	) -> Result<ParsedMessagesDeliveryProofFromBridgedChain<B>, &'static str> {
		// Messages delivery proof is just proof of single storage key read => any error
		// is fatal.
		let storage_inbound_lane_data_key = bp_messages::storage_keys::inbound_lane_data_key(
			B::BRIDGED_MESSAGES_PALLET_NAME,
			&lane,
		);
		let raw_inbound_lane_data = storage
			.read_value(storage_inbound_lane_data_key.0.as_ref())
			.map_err(|_| "Failed to read inbound lane state from storage proof")?
			.ok_or("Inbound lane state is missing from the messages proof")?;
		let inbound_lane_data = InboundLaneData::decode(&mut &raw_inbound_lane_data[..])
			.map_err(|_| "Failed to decode inbound lane state from the proof")?;

		Ok((lane, inbound_lane_data))
	}
}

/// Sub-module that is declaring types required for processing Bridged -> This chain messages.
pub mod target {
	use super::*;
	use frame_support::log;

	/// Decoded Bridged -> This message payload.
	#[derive(RuntimeDebug, PartialEq)]
	pub struct FromBridgedChainMessagePayload<Call> {
		/// Data that is actually sent over the wire.
		pub xcm: (xcm::v3::MultiLocation, xcm::v3::Xcm<Call>),
		/// Weight of the message, computed by the weigher. Unknown initially.
		pub weight: Option<Weight>,
	}

	impl<Call: Decode> Decode for FromBridgedChainMessagePayload<Call> {
		fn decode<I: codec::Input>(input: &mut I) -> Result<Self, codec::Error> {
			let _: codec::Compact<u32> = Decode::decode(input)?;
			type XcmPairType<Call> = (xcm::v3::MultiLocation, xcm::v3::Xcm<Call>);
			Ok(FromBridgedChainMessagePayload {
				xcm: XcmPairType::<Call>::decode_with_depth_limit(
					sp_api::MAX_EXTRINSIC_DEPTH,
					input,
				)?,
				weight: None,
			})
		}
	}

	impl<Call> From<(xcm::v3::MultiLocation, xcm::v3::Xcm<Call>)>
		for FromBridgedChainMessagePayload<Call>
	{
		fn from(xcm: (xcm::v3::MultiLocation, xcm::v3::Xcm<Call>)) -> Self {
			FromBridgedChainMessagePayload { xcm, weight: None }
		}
	}

	/// Messages proof from bridged chain:
	///
	/// - hash of finalized header;
	/// - storage proof of messages and (optionally) outbound lane state;
	/// - lane id;
	/// - nonces (inclusive range) of messages which are included in this proof.
	#[derive(Clone, Encode, PartialEq, Eq, Decode, RuntimeDebug, TypeInfo)]
	pub struct FromBridgedChainMessagesProof<BridgedHeaderHash> {
		/// Hash of the finalized bridged header the proof is for.
		pub bridged_header_hash: BridgedHeaderHash,
		/// A storage trie proof of messages being delivered.
		pub storage_proof: RawStorageProof,
		/// Messages in this proof are sent over this lane.
		pub lane: LaneId,
		/// Nonce of the first message being delivered.
		pub nonces_start: MessageNonce,
		/// Nonce of the last message being delivered.
		pub nonces_end: MessageNonce,
	}
	impl<BridgedHeaderHash> Size for FromBridgedChainMessagesProof<BridgedHeaderHash> {
		fn size(&self) -> u32 {
			u32::try_from(
				self.storage_proof.iter().fold(0usize, |sum, node| sum.saturating_add(node.len())),
			)
			.unwrap_or(u32::MAX)
		}
	}

	/// Encoded Call of This chain as it is transferred over bridge.
	///
	/// Our Call is opaque (`Vec<u8>`) for Bridged chain. So it is encoded, prefixed with
	/// vector length. Custom decode implementation here is exactly to deal with this.
	#[derive(Clone, PartialEq, Eq, Encode, Decode, RuntimeDebug)]
	pub struct FromBridgedChainEncodedMessageCall<DecodedCall> {
		encoded_call: Vec<u8>,
		_marker: PhantomData<DecodedCall>,
	}
	impl<DecodedCall> FromBridgedChainEncodedMessageCall<DecodedCall> {
		/// Create encoded call.
		pub fn new(encoded_call: Vec<u8>) -> Self {
			FromBridgedChainEncodedMessageCall { encoded_call, _marker: PhantomData::default() }
		}
	}

	/// Dispatching Bridged -> This chain messages.
	#[derive(Clone, Copy, RuntimeDebug)]
	pub struct FromBridgedChainMessageDispatch<B, XcmExecutor, XcmWeigher, WeightCredit> {
		_marker: PhantomData<(B, XcmExecutor, XcmWeigher, WeightCredit)>,
	}
	impl<B: MessageBridge, XcmExecutor, XcmWeigher, WeightCredit>
		MessageDispatch<AccountIdOf<ThisChain<B>>, BalanceOf<BridgedChain<B>>>
		for FromBridgedChainMessageDispatch<B, XcmExecutor, XcmWeigher, WeightCredit>
	where
		XcmExecutor: xcm::v3::ExecuteXcm<CallOf<ThisChain<B>>>,
		XcmWeigher: xcm_executor::traits::WeightBounds<CallOf<ThisChain<B>>>,
		WeightCredit: Get<Weight>,
	{
		type DispatchPayload = FromBridgedChainMessagePayload<CallOf<ThisChain<B>>>;

		fn dispatch_weight(
			message: &mut DispatchMessage<Self::DispatchPayload, BalanceOf<BridgedChain<B>>>,
		) -> frame_support::weights::Weight {
			match message.data.payload {
				Ok(ref mut payload) => {
					// I have no idea why this method takes `&mut` reference and there's nothing
					// about that in documentation. Hope it'll only mutate iff error is returned.
					let weight = XcmWeigher::weight(&mut payload.xcm.1);
					let weight = weight.unwrap_or_else(|e| {
						log::debug!(
							target: "runtime::bridge-dispatch",
							"Failed to compute dispatch weight of incoming XCM message {:?}/{}: {:?}",
							message.key.lane_id,
							message.key.nonce,
							e,
						);

						// we shall return 0 and then the XCM executor will fail to execute XCM
						// if we'll return something else (e.g. maximal value), the lane may stuck
						0
					});

					payload.weight = Some(weight);
					weight
				},
				_ => 0,
			}
		}

		fn pre_dispatch(
			_relayer_account: &AccountIdOf<ThisChain<B>>,
			_message: &DispatchMessage<Self::DispatchPayload, BalanceOf<BridgedChain<B>>>,
		) -> Result<(), &'static str> {
			unimplemented!("TODO")
		}

		fn dispatch(
			_relayer_account: &AccountIdOf<ThisChain<B>>,
			message: DispatchMessage<Self::DispatchPayload, BalanceOf<BridgedChain<B>>>,
		) -> MessageDispatchResult {
			use xcm::latest::*;

			let message_id = (message.key.lane_id, message.key.nonce);
			let do_dispatch = move || -> sp_std::result::Result<Outcome, codec::Error> {
				let FromBridgedChainMessagePayload { xcm: (location, xcm), weight: weight_limit } =
					message.data.payload?;
				log::trace!(
					target: "runtime::bridge-dispatch",
					"Going to execute message {:?} (weight limit: {:?}): {:?} {:?}",
					message_id,
					weight_limit,
					location,
					xcm,
				);
				let hash = message_id.using_encoded(sp_io::hashing::blake2_256);

				// if this cod will end up in production, this most likely needs to be set to zero
				let weight_credit = WeightCredit::get();

				let xcm_outcome = XcmExecutor::execute_xcm_in_credit(
					location,
					xcm,
					hash,
					weight_limit.unwrap_or(0),
					weight_credit,
				);
				Ok(xcm_outcome)
			};

			let xcm_outcome = do_dispatch();
			log::trace!(target: "runtime::bridge-dispatch", "Incoming message {:?} dispatched with result: {:?}", message_id, xcm_outcome);
			MessageDispatchResult {
				dispatch_result: true,
				unspent_weight: 0,
				dispatch_fee_paid_during_dispatch: false,
			}
		}
	}

	impl<DecodedCall: Decode> From<FromBridgedChainEncodedMessageCall<DecodedCall>>
		for Result<DecodedCall, ()>
	{
		fn from(encoded_call: FromBridgedChainEncodedMessageCall<DecodedCall>) -> Self {
			DecodedCall::decode_with_depth_limit(
				sp_api::MAX_EXTRINSIC_DEPTH,
				&mut &encoded_call.encoded_call[..],
			)
			.map_err(drop)
		}
	}

	/// Return maximal dispatch weight of the message we're able to receive.
	pub fn maximal_incoming_message_dispatch_weight(maximal_extrinsic_weight: Weight) -> Weight {
		maximal_extrinsic_weight / 2
	}

	/// Return maximal message size given maximal extrinsic size.
	pub fn maximal_incoming_message_size(maximal_extrinsic_size: u32) -> u32 {
		maximal_extrinsic_size / 3 * 2
	}

	/// Verify proof of Bridged -> This chain messages.
	///
	/// This function is used when Bridged chain is directly using GRANDPA finality. For Bridged
	/// parachains, please use the `verify_messages_proof_from_parachain`.
	///
	/// The `messages_count` argument verification (sane limits) is supposed to be made
	/// outside of this function. This function only verifies that the proof declares exactly
	/// `messages_count` messages.
	pub fn verify_messages_proof<B: MessageBridge, ThisRuntime, GrandpaInstance: 'static>(
		proof: FromBridgedChainMessagesProof<HashOf<BridgedChain<B>>>,
		messages_count: u32,
	) -> Result<ProvedMessages<Message<BalanceOf<BridgedChain<B>>>>, &'static str>
	where
		ThisRuntime: pallet_bridge_grandpa::Config<GrandpaInstance>,
		HashOf<BridgedChain<B>>: Into<
			bp_runtime::HashOf<
				<ThisRuntime as pallet_bridge_grandpa::Config<GrandpaInstance>>::BridgedChain,
			>,
		>,
	{
		verify_messages_proof_with_parser::<B, _, _>(
			proof,
			messages_count,
			|bridged_header_hash, bridged_storage_proof| {
				pallet_bridge_grandpa::Pallet::<ThisRuntime, GrandpaInstance>::parse_finalized_storage_proof(
					bridged_header_hash.into(),
					StorageProof::new(bridged_storage_proof),
					|storage_adapter| storage_adapter,
				)
				.map(|storage| StorageProofCheckerAdapter::<_, B> {
					storage,
					_dummy: Default::default(),
				})
				.map_err(|err| MessageProofError::Custom(err.into()))
			},
		)
		.map_err(Into::into)
	}

	/// Verify proof of Bridged -> This chain messages.
	///
	/// This function is used when Bridged chain is using parachain finality. For Bridged
	/// chains with direct GRANDPA finality, please use the `verify_messages_proof`.
	///
	/// The `messages_count` argument verification (sane limits) is supposed to be made
	/// outside of this function. This function only verifies that the proof declares exactly
	/// `messages_count` messages.
	///
	/// This function currently only supports parachains, which are using header type that
	/// implements `sp_runtime::traits::Header` trait.
	pub fn verify_messages_proof_from_parachain<
		B,
		BridgedHeader,
		ThisRuntime,
		ParachainsInstance: 'static,
	>(
		bridged_parachain: ParaId,
		proof: FromBridgedChainMessagesProof<HashOf<BridgedChain<B>>>,
		messages_count: u32,
	) -> Result<ProvedMessages<Message<BalanceOf<BridgedChain<B>>>>, &'static str>
	where
		B: MessageBridge,
		B::BridgedChain: ChainWithMessages<Hash = ParaHash>,
		BridgedHeader: HeaderT<Hash = HashOf<BridgedChain<B>>>,
		ThisRuntime: pallet_bridge_parachains::Config<ParachainsInstance>,
	{
		verify_messages_proof_with_parser::<B, _, _>(
			proof,
			messages_count,
			|bridged_header_hash, bridged_storage_proof| {
				pallet_bridge_parachains::Pallet::<ThisRuntime, ParachainsInstance>::parse_finalized_storage_proof(
					bridged_parachain,
					bridged_header_hash,
					StorageProof::new(bridged_storage_proof),
					|para_head| BridgedHeader::decode(&mut &para_head.0[..]).ok().map(|h| *h.state_root()),
					|storage_adapter| storage_adapter,
				)
				.map(|storage| StorageProofCheckerAdapter::<_, B> {
					storage,
					_dummy: Default::default(),
				})
				.map_err(|err| MessageProofError::Custom(err.into()))
			},
		)
		.map_err(Into::into)
	}

	#[derive(Debug, PartialEq, Eq)]
	pub(crate) enum MessageProofError {
		Empty,
		MessagesCountMismatch,
		MissingRequiredMessage,
		FailedToDecodeMessage,
		FailedToDecodeOutboundLaneState,
		Custom(&'static str),
	}

	impl From<MessageProofError> for &'static str {
		fn from(err: MessageProofError) -> &'static str {
			match err {
				MessageProofError::Empty => "Messages proof is empty",
				MessageProofError::MessagesCountMismatch =>
					"Declared messages count doesn't match actual value",
				MessageProofError::MissingRequiredMessage => "Message is missing from the proof",
				MessageProofError::FailedToDecodeMessage =>
					"Failed to decode message from the proof",
				MessageProofError::FailedToDecodeOutboundLaneState =>
					"Failed to decode outbound lane data from the proof",
				MessageProofError::Custom(err) => err,
			}
		}
	}

	pub(crate) trait MessageProofParser {
		fn read_raw_outbound_lane_data(&self, lane_id: &LaneId) -> Option<Vec<u8>>;
		fn read_raw_message(&self, message_key: &MessageKey) -> Option<Vec<u8>>;
	}

	struct StorageProofCheckerAdapter<H: Hasher, B> {
		storage: StorageProofChecker<H>,
		_dummy: sp_std::marker::PhantomData<B>,
	}

	impl<H, B> MessageProofParser for StorageProofCheckerAdapter<H, B>
	where
		H: Hasher,
		B: MessageBridge,
	{
		fn read_raw_outbound_lane_data(&self, lane_id: &LaneId) -> Option<Vec<u8>> {
			let storage_outbound_lane_data_key = bp_messages::storage_keys::outbound_lane_data_key(
				B::BRIDGED_MESSAGES_PALLET_NAME,
				lane_id,
			);
			self.storage.read_value(storage_outbound_lane_data_key.0.as_ref()).ok()?
		}

		fn read_raw_message(&self, message_key: &MessageKey) -> Option<Vec<u8>> {
			let storage_message_key = bp_messages::storage_keys::message_key(
				B::BRIDGED_MESSAGES_PALLET_NAME,
				&message_key.lane_id,
				message_key.nonce,
			);
			self.storage.read_value(storage_message_key.0.as_ref()).ok()?
		}
	}

	/// Verify proof of Bridged -> This chain messages using given message proof parser.
	pub(crate) fn verify_messages_proof_with_parser<B: MessageBridge, BuildParser, Parser>(
		proof: FromBridgedChainMessagesProof<HashOf<BridgedChain<B>>>,
		messages_count: u32,
		build_parser: BuildParser,
	) -> Result<ProvedMessages<Message<BalanceOf<BridgedChain<B>>>>, MessageProofError>
	where
		BuildParser:
			FnOnce(HashOf<BridgedChain<B>>, RawStorageProof) -> Result<Parser, MessageProofError>,
		Parser: MessageProofParser,
	{
		let FromBridgedChainMessagesProof {
			bridged_header_hash,
			storage_proof,
			lane,
			nonces_start,
			nonces_end,
		} = proof;

		// receiving proofs where end < begin is ok (if proof includes outbound lane state)
		let messages_in_the_proof =
			if let Some(nonces_difference) = nonces_end.checked_sub(nonces_start) {
				// let's check that the user (relayer) has passed correct `messages_count`
				// (this bounds maximal capacity of messages vec below)
				let messages_in_the_proof = nonces_difference.saturating_add(1);
				if messages_in_the_proof != MessageNonce::from(messages_count) {
					return Err(MessageProofError::MessagesCountMismatch);
				}

				messages_in_the_proof
			} else {
				0
			};

		let parser = build_parser(bridged_header_hash, storage_proof)?;

		// Read messages first. All messages that are claimed to be in the proof must
		// be in the proof. So any error in `read_value`, or even missing value is fatal.
		//
		// Mind that we allow proofs with no messages if outbound lane state is proved.
		let mut messages = Vec::with_capacity(messages_in_the_proof as _);
		for nonce in nonces_start..=nonces_end {
			let message_key = MessageKey { lane_id: lane, nonce };
			let raw_message_data = parser
				.read_raw_message(&message_key)
				.ok_or(MessageProofError::MissingRequiredMessage)?;
			let message_data =
				MessageData::<BalanceOf<BridgedChain<B>>>::decode(&mut &raw_message_data[..])
					.map_err(|_| MessageProofError::FailedToDecodeMessage)?;
			messages.push(Message { key: message_key, data: message_data });
		}

		// Now let's check if proof contains outbound lane state proof. It is optional, so we
		// simply ignore `read_value` errors and missing value.
		let mut proved_lane_messages = ProvedLaneMessages { lane_state: None, messages };
		let raw_outbound_lane_data = parser.read_raw_outbound_lane_data(&lane);
		if let Some(raw_outbound_lane_data) = raw_outbound_lane_data {
			proved_lane_messages.lane_state = Some(
				OutboundLaneData::decode(&mut &raw_outbound_lane_data[..])
					.map_err(|_| MessageProofError::FailedToDecodeOutboundLaneState)?,
			);
		}

		// Now we may actually check if the proof is empty or not.
		if proved_lane_messages.lane_state.is_none() && proved_lane_messages.messages.is_empty() {
			return Err(MessageProofError::Empty);
		}

		// We only support single lane messages in this generated_schema
		let mut proved_messages = ProvedMessages::new();
		proved_messages.insert(lane, proved_lane_messages);

		Ok(proved_messages)
	}
}

pub use xcm_copy::*;

// copy of private types from xcm-builder/src/universal_exports.rs
pub mod xcm_copy {
	use codec::{Decode, Encode};
	use frame_support::{ensure, traits::Get};
	use sp_std::{convert::TryInto, marker::PhantomData, prelude::*};
	use xcm::prelude::*;
	use xcm_executor::traits::ExportXcm;

	pub trait DispatchBlob {
		/// Dispatches an incoming blob and returns the unexpectable weight consumed by the
		/// dispatch.
		fn dispatch_blob(blob: Vec<u8>) -> Result<(), DispatchBlobError>;
	}

	pub trait HaulBlob {
		/// Sends a blob over some point-to-point link. This will generally be implemented by a
		/// bridge.
		fn haul_blob(blob: Vec<u8>);
	}

	#[derive(Clone, Encode, Decode)]
	pub struct BridgeMessage {
		/// The message destination as a *Universal Location*. This means it begins with a
		/// `GlobalConsensus` junction describing the network under which global consensus happens.
		/// If this does not match our global consensus then it's a fatal error.
		universal_dest: VersionedInteriorMultiLocation,
		message: VersionedXcm<()>,
	}

	pub enum DispatchBlobError {
		Unbridgable,
		InvalidEncoding,
		UnsupportedLocationVersion,
		UnsupportedXcmVersion,
		RoutingError,
		NonUniversalDestination,
		WrongGlobal,
	}

	pub struct BridgeBlobDispatcher<Router, OurPlace>(PhantomData<(Router, OurPlace)>);
	impl<Router: SendXcm, OurPlace: Get<InteriorMultiLocation>> DispatchBlob
		for BridgeBlobDispatcher<Router, OurPlace>
	{
		fn dispatch_blob(blob: Vec<u8>) -> Result<(), DispatchBlobError> {
			let our_universal = OurPlace::get();
			let our_global =
				our_universal.global_consensus().map_err(|()| DispatchBlobError::Unbridgable)?;
			let BridgeMessage { universal_dest, message } =
				Decode::decode(&mut &blob[..]).map_err(|_| DispatchBlobError::InvalidEncoding)?;
			let universal_dest: InteriorMultiLocation = universal_dest
				.try_into()
				.map_err(|_| DispatchBlobError::UnsupportedLocationVersion)?;
			// `universal_dest` is the desired destination within the universe: first we need to
			// check we're in the right global consensus.
			let intended_global = universal_dest
				.global_consensus()
				.map_err(|()| DispatchBlobError::NonUniversalDestination)?;
			ensure!(intended_global == our_global, DispatchBlobError::WrongGlobal);
			let dest = universal_dest.relative_to(&our_universal);
			let message: Xcm<()> =
				message.try_into().map_err(|_| DispatchBlobError::UnsupportedXcmVersion)?;
			send_xcm::<Router>(dest, message).map_err(|_| DispatchBlobError::RoutingError)?;
			Ok(())
		}
	}

	pub struct HaulBlobExporter<Bridge, BridgedNetwork, Price>(
		PhantomData<(Bridge, BridgedNetwork, Price)>,
	);
	impl<Bridge: HaulBlob, BridgedNetwork: Get<NetworkId>, Price: Get<MultiAssets>> ExportXcm
		for HaulBlobExporter<Bridge, BridgedNetwork, Price>
	{
		type Ticket = (Vec<u8>, XcmHash);

		fn validate(
			network: NetworkId,
			_channel: u32,
			destination: &mut Option<InteriorMultiLocation>,
			message: &mut Option<Xcm<()>>,
		) -> Result<((Vec<u8>, XcmHash), MultiAssets), SendError> {
			let bridged_network = BridgedNetwork::get();
			ensure!(&network == &bridged_network, SendError::NotApplicable);
			// We don't/can't use the `channel` for this adapter.
			let dest = destination.take().ok_or(SendError::MissingArgument)?;
			let universal_dest = match dest.pushed_front_with(GlobalConsensus(bridged_network)) {
				Ok(d) => d.into(),
				Err((dest, _)) => {
					*destination = Some(dest);
					return Err(SendError::NotApplicable);
				},
			};
			let message = VersionedXcm::from(message.take().ok_or(SendError::MissingArgument)?);
			let hash = message.using_encoded(sp_io::hashing::blake2_256);
			let blob = BridgeMessage { universal_dest, message }.encode();
			Ok(((blob, hash), Price::get()))
		}

		fn deliver((blob, hash): (Vec<u8>, XcmHash)) -> Result<XcmHash, SendError> {
			Bridge::haul_blob(blob);
			Ok(hash)
		}
	}
}

#[cfg(test)]
mod tests {
	// std
	use std::ops::RangeInclusive;
	// crates.io
	use codec::{Decode, Encode};
	// darwinia-network
	use super::*;
	use bp_runtime::messages::DispatchFeePayment;
	// paritytech
	use frame_support::weights::Weight;

	// const DELIVERY_TRANSACTION_WEIGHT: Weight = 100;
	// const DELIVERY_CONFIRMATION_TRANSACTION_WEIGHT: Weight = 100;
	// const THIS_CHAIN_WEIGHT_TO_BALANCE_RATE: Weight = 2;
	// const BRIDGED_CHAIN_WEIGHT_TO_BALANCE_RATE: Weight = 4;
	// const BRIDGED_CHAIN_TO_THIS_CHAIN_BALANCE_RATE: u32 = 6;
	// const BRIDGED_CHAIN_MIN_EXTRINSIC_WEIGHT: usize = 5;
	const BRIDGED_CHAIN_MAX_EXTRINSIC_WEIGHT: Weight = 2048;
	const BRIDGED_CHAIN_MAX_EXTRINSIC_SIZE: u32 = 1024;

	const TEST_LANE_ID: &LaneId = b"test";
	const MAXIMAL_PENDING_MESSAGES_AT_TEST_LANE: MessageNonce = 32;

	/// Bridge that is deployed on ThisChain and allows sending/receiving messages to/from
	/// BridgedChain;
	#[derive(Debug, PartialEq, Eq)]
	struct OnThisChainBridge;

	impl MessageBridge for OnThisChainBridge {
		type BridgedChain = BridgedChain;
		type ThisChain = ThisChain;

		const BRIDGED_CHAIN_ID: ChainId = *b"brdg";
		const BRIDGED_MESSAGES_PALLET_NAME: &'static str = "";
		const RELAYER_FEE_PERCENT: u32 = 10;
		const THIS_CHAIN_ID: ChainId = *b"this";
	}

	/// Bridge that is deployed on BridgedChain and allows sending/receiving messages to/from
	/// ThisChain;
	#[derive(Debug, PartialEq, Eq)]
	struct OnBridgedChainBridge;

	impl MessageBridge for OnBridgedChainBridge {
		type BridgedChain = ThisChain;
		type ThisChain = BridgedChain;

		const BRIDGED_CHAIN_ID: ChainId = *b"this";
		const BRIDGED_MESSAGES_PALLET_NAME: &'static str = "";
		const RELAYER_FEE_PERCENT: u32 = 20;
		const THIS_CHAIN_ID: ChainId = *b"brdg";
	}

	#[derive(Debug, PartialEq, Eq, Encode, Decode, Clone, MaxEncodedLen)]
	struct ThisChainAccountId(u32);
	#[derive(Debug, PartialEq, Eq, Encode, Decode)]
	struct ThisChainSigner(u32);
	#[derive(Debug, PartialEq, Eq, Encode, Decode)]
	struct ThisChainSignature(u32);
	#[derive(Debug, PartialEq, Eq, Encode, Decode)]
	enum ThisChainCall {
		#[codec(index = 42)]
		Transfer,
		#[codec(index = 84)]
		Mint,
	}
	#[derive(Clone, Debug)]
	struct ThisChainOrigin(Result<frame_system::RawOrigin<ThisChainAccountId>, ()>);
	impl From<ThisChainOrigin>
		for Result<frame_system::RawOrigin<ThisChainAccountId>, ThisChainOrigin>
	{
		fn from(
			origin: ThisChainOrigin,
		) -> Result<frame_system::RawOrigin<ThisChainAccountId>, ThisChainOrigin> {
			origin.clone().0.map_err(|_| origin)
		}
	}

	#[derive(Debug, PartialEq, Eq, Encode, Decode, MaxEncodedLen)]
	struct BridgedChainAccountId(u32);
	#[derive(Debug, PartialEq, Eq, Encode, Decode)]
	struct BridgedChainSigner(u32);
	#[derive(Debug, PartialEq, Eq, Encode, Decode)]
	struct BridgedChainSignature(u32);
	#[derive(Debug, PartialEq, Eq, Encode, Decode)]
	enum BridgedChainCall {}
	#[derive(Clone, Debug)]
	struct BridgedChainOrigin;
	impl From<BridgedChainOrigin>
		for Result<frame_system::RawOrigin<BridgedChainAccountId>, BridgedChainOrigin>
	{
		fn from(
			_origin: BridgedChainOrigin,
		) -> Result<frame_system::RawOrigin<BridgedChainAccountId>, BridgedChainOrigin> {
			unreachable!()
		}
	}

	macro_rules! impl_wrapped_balance {
		($name:ident) => {
			#[derive(Clone, Copy, Debug, PartialEq, Eq, Encode, Decode)]
			struct $name(u32);

			impl From<u32> for $name {
				fn from(balance: u32) -> Self {
					Self(balance)
				}
			}

			impl sp_std::ops::Add for $name {
				type Output = $name;

				fn add(self, other: Self) -> Self {
					Self(self.0 + other.0)
				}
			}

			impl sp_std::ops::Div for $name {
				type Output = $name;

				fn div(self, other: Self) -> Self {
					Self(self.0 / other.0)
				}
			}

			impl sp_std::ops::Mul for $name {
				type Output = $name;

				fn mul(self, other: Self) -> Self {
					Self(self.0 * other.0)
				}
			}

			impl sp_std::cmp::PartialOrd for $name {
				fn partial_cmp(&self, other: &Self) -> Option<sp_std::cmp::Ordering> {
					self.0.partial_cmp(&other.0)
				}
			}

			impl CheckedAdd for $name {
				fn checked_add(&self, other: &Self) -> Option<Self> {
					self.0.checked_add(other.0).map(Self)
				}
			}

			impl CheckedDiv for $name {
				fn checked_div(&self, other: &Self) -> Option<Self> {
					self.0.checked_div(other.0).map(Self)
				}
			}

			impl CheckedMul for $name {
				fn checked_mul(&self, other: &Self) -> Option<Self> {
					self.0.checked_mul(other.0).map(Self)
				}
			}
		};
	}

	impl_wrapped_balance!(ThisChainBalance);
	impl_wrapped_balance!(BridgedChainBalance);

	struct ThisChain;
	impl ChainWithMessages for ThisChain {
		type AccountId = ThisChainAccountId;
		type Balance = ThisChainBalance;
		type Hash = ();
		type Signature = ThisChainSignature;
		type Signer = ThisChainSigner;
		type Weight = frame_support::weights::Weight;
	}
	impl ThisChainWithMessages for ThisChain {
		type Call = ThisChainCall;
		type Origin = ThisChainOrigin;

		fn is_message_accepted(_send_origin: &Self::Origin, lane: &LaneId) -> bool {
			lane == TEST_LANE_ID
		}

		fn maximal_pending_messages_at_outbound_lane() -> MessageNonce {
			MAXIMAL_PENDING_MESSAGES_AT_TEST_LANE
		}
	}
	impl BridgedChainWithMessages for ThisChain {
		fn maximal_extrinsic_size() -> u32 {
			unreachable!()
		}

		fn verify_dispatch_weight(_message_payload: &[u8], _payload_weight: &Weight) -> bool {
			unreachable!()
		}
	}

	struct BridgedChain;
	impl ChainWithMessages for BridgedChain {
		type AccountId = BridgedChainAccountId;
		type Balance = BridgedChainBalance;
		type Hash = ();
		type Signature = BridgedChainSignature;
		type Signer = BridgedChainSigner;
		type Weight = frame_support::weights::Weight;
	}
	impl ThisChainWithMessages for BridgedChain {
		type Call = BridgedChainCall;
		type Origin = BridgedChainOrigin;

		fn is_message_accepted(_send_origin: &Self::Origin, _lane: &LaneId) -> bool {
			unreachable!()
		}

		fn maximal_pending_messages_at_outbound_lane() -> MessageNonce {
			unreachable!()
		}
	}
	impl BridgedChainWithMessages for BridgedChain {
		fn maximal_extrinsic_size() -> u32 {
			BRIDGED_CHAIN_MAX_EXTRINSIC_SIZE
		}

		fn verify_dispatch_weight(message_payload: &[u8], payload_weight: &Weight) -> bool {
			let begin =
				std::cmp::min(BRIDGED_CHAIN_MAX_EXTRINSIC_WEIGHT, message_payload.len() as Weight);
			(begin..=BRIDGED_CHAIN_MAX_EXTRINSIC_WEIGHT).contains(payload_weight)
		}
	}

	// fn test_lane_outbound_data() -> OutboundLaneData {
	// 	OutboundLaneData::default()
	// }

	fn regular_outbound_message_payload() -> source::FromThisChainMessagePayload {
		vec![42]
	}

	#[test]
	fn message_from_bridged_chain_is_decoded() {
		// // the message is encoded on the bridged chain
		// let message_on_bridged_chain = source::FromThisChainMessagePayload {
		// 	spec_version: 1,
		// 	weight: 100,
		// 	origin: bp_message_dispatch::CallOrigin::SourceRoot,
		// 	dispatch_fee_payment: DispatchFeePayment::AtTargetChain,
		// 	call: ThisChainCall::Transfer.encode(),
		// }
		// .encode();
		//
		// // and sent to this chain where it is decoded
		// let message_on_this_chain =
		// 	target::FromBridgedChainMessagePayload::<OnThisChainBridge>::decode(
		// 		&mut &message_on_bridged_chain[..],
		// 	)
		// 	.unwrap();
		// assert_eq!(
		// 	message_on_this_chain,
		// 	target::FromBridgedChainMessagePayload::<OnThisChainBridge> {
		// 		spec_version: 1,
		// 		weight: 100,
		// 		origin: bp_message_dispatch::CallOrigin::SourceRoot,
		// 		dispatch_fee_payment: DispatchFeePayment::AtTargetChain,
		// 		call: target::FromBridgedChainEncodedMessageCall::<ThisChainCall>::new(
		// 			ThisChainCall::Transfer.encode(),
		// 		),
		// 	}
		// );
		// assert_eq!(Ok(ThisChainCall::Transfer), message_on_this_chain.call.into());
		unimplemented!("TODO")
	}

	// #[test]
	// fn message_fee_is_checked_by_verifier() {
	// 	const EXPECTED_MINIMAL_FEE: u32 = 2860;

	// 	// payload of the This -> Bridged chain message
	// 	let payload = regular_outbound_message_payload();

	// 	// and now check that the verifier checks the fee
	// 	assert_eq!(
	// 		source::FromThisChainMessageVerifier::<OnThisChainBridge>::verify_message(
	// 			&ThisChainOrigin(Ok(frame_system::RawOrigin::Root)),
	// 			&ThisChainBalance(1),
	// 			TEST_LANE_ID,
	// 			&test_lane_outbound_data(),
	// 			&payload,
	// 		),
	// 		Err(source::TOO_LOW_FEE)
	// 	);
	// 	assert!(source::FromThisChainMessageVerifier::<OnThisChainBridge>::verify_message(
	// 		&ThisChainOrigin(Ok(frame_system::RawOrigin::Root)),
	// 		&ThisChainBalance(1_000_000),
	// 		TEST_LANE_ID,
	// 		&test_lane_outbound_data(),
	// 		&payload,
	// 	)
	// 	.is_ok(),);
	// }

	// #[test]
	// fn message_is_rejected_when_sent_using_disabled_lane() {
	// 	assert_eq!(
	// 		source::FromThisChainMessageVerifier::<OnThisChainBridge>::verify_message(
	// 			&ThisChainOrigin(Ok(frame_system::RawOrigin::Root)),
	// 			&ThisChainBalance(1_000_000),
	// 			b"dsbl",
	// 			&test_lane_outbound_data(),
	// 			&regular_outbound_message_payload(),
	// 		),
	// 		Err(source::MESSAGE_REJECTED_BY_OUTBOUND_LANE)
	// 	);
	// }

	// #[test]
	// fn message_is_rejected_when_there_are_too_many_pending_messages_at_outbound_lane() {
	// 	assert_eq!(
	// 		source::FromThisChainMessageVerifier::<OnThisChainBridge>::verify_message(
	// 			&ThisChainOrigin(Ok(frame_system::RawOrigin::Root)),
	// 			&ThisChainBalance(1_000_000),
	// 			TEST_LANE_ID,
	// 			&OutboundLaneData {
	// 				latest_received_nonce: 100,
	// 				latest_generated_nonce: 100 + MAXIMAL_PENDING_MESSAGES_AT_TEST_LANE + 1,
	// 				..Default::default()
	// 			},
	// 			&regular_outbound_message_payload(),
	// 		),
	// 		Err(source::TOO_MANY_PENDING_MESSAGES)
	// 	);
	// }

	#[test]
	fn verify_chain_message_rejects_message_with_too_small_declared_weight() {
		// assert!(source::verify_chain_message::<OnThisChainBridge>(
		// 	&source::FromThisChainMessagePayload::<OnThisChainBridge> {
		// 		spec_version: 1,
		// 		weight: 5,
		// 		origin: bp_message_dispatch::CallOrigin::SourceRoot,
		// 		dispatch_fee_payment: DispatchFeePayment::AtSourceChain,
		// 		call: vec![1, 2, 3, 4, 5, 6],
		// 	},
		// )
		// .is_err());
		unimplemented!("TODO")
	}

	#[test]
	fn verify_chain_message_rejects_message_with_too_large_declared_weight() {
		// assert!(source::verify_chain_message::<OnThisChainBridge>(
		// 	&source::FromThisChainMessagePayload::<OnThisChainBridge> {
		// 		spec_version: 1,
		// 		weight: BRIDGED_CHAIN_MAX_EXTRINSIC_WEIGHT + 1,
		// 		origin: bp_message_dispatch::CallOrigin::SourceRoot,
		// 		dispatch_fee_payment: DispatchFeePayment::AtSourceChain,
		// 		call: vec![1, 2, 3, 4, 5, 6],
		// 	},
		// )
		// .is_err());
		unimplemented!("TODO")
	}

	// #[test]
	// fn verify_chain_message_rejects_message_too_large_message() {
	// 	assert!(source::verify_chain_message::<OnThisChainBridge>(
	// 		vec![0; source::maximal_message_size::<OnThisChainBridge>() as usize + 1],
	// 	)
	// 	.is_err());
	// }
	//
	// #[test]
	// fn verify_chain_message_accepts_maximal_message() {
	// 	assert_eq!(
	// 		source::verify_chain_message::<OnThisChainBridge>(
	// 			vec![0; source::maximal_message_size::<OnThisChainBridge>() as _],
	// 		),
	// 		Ok(()),
	// 	);
	// }
	//
	// #[derive(Debug)]
	// struct TestMessageProofParser {
	// 	failing: bool,
	// 	messages: RangeInclusive<MessageNonce>,
	// 	outbound_lane_data: Option<OutboundLaneData>,
	// }
	//
	// impl target::MessageProofParser for TestMessageProofParser {
	// 	fn read_raw_outbound_lane_data(&self, _lane_id: &LaneId) -> Option<Vec<u8>> {
	// 		if self.failing {
	// 			Some(vec![])
	// 		} else {
	// 			self.outbound_lane_data.clone().map(|data| data.encode())
	// 		}
	// 	}
	//
	// 	fn read_raw_message(&self, message_key: &MessageKey) -> Option<Vec<u8>> {
	// 		if self.failing {
	// 			Some(vec![])
	// 		} else if self.messages.contains(&message_key.nonce) {
	// 			Some(
	// 				MessageData::<BridgedChainBalance> {
	// 					payload: message_key.nonce.encode(),
	// 					fee: BridgedChainBalance(0),
	// 				}
	// 				.encode(),
	// 			)
	// 		} else {
	// 			None
	// 		}
	// 	}
	// }
	//
	// #[allow(clippy::reversed_empty_ranges)]
	// fn no_messages_range() -> RangeInclusive<MessageNonce> {
	// 	1..=0
	// }
	//
	// fn messages_proof(nonces_end: MessageNonce) -> target::FromBridgedChainMessagesProof<()> {
	// 	target::FromBridgedChainMessagesProof {
	// 		bridged_header_hash: (),
	// 		storage_proof: vec![],
	// 		lane: Default::default(),
	// 		nonces_start: 1,
	// 		nonces_end,
	// 	}
	// }
	//
	// #[test]
	// fn messages_proof_is_rejected_if_declared_less_than_actual_number_of_messages() {
	// 	assert_eq!(
	// 		target::verify_messages_proof_with_parser::<OnThisChainBridge, _, TestMessageProofParser>(
	// 			messages_proof(10),
	// 			5,
	// 			|_, _| unreachable!(),
	// 		),
	// 		Err(target::MessageProofError::MessagesCountMismatch),
	// 	);
	// }
	//
	// #[test]
	// fn messages_proof_is_rejected_if_declared_more_than_actual_number_of_messages() {
	// 	assert_eq!(
	// 		target::verify_messages_proof_with_parser::<OnThisChainBridge, _, TestMessageProofParser>(
	// 			messages_proof(10),
	// 			15,
	// 			|_, _| unreachable!(),
	// 		),
	// 		Err(target::MessageProofError::MessagesCountMismatch),
	// 	);
	// }
	//
	// #[test]
	// fn message_proof_is_rejected_if_build_parser_fails() {
	// 	assert_eq!(
	// 		target::verify_messages_proof_with_parser::<OnThisChainBridge, _, TestMessageProofParser>(
	// 			messages_proof(10),
	// 			10,
	// 			|_, _| Err(target::MessageProofError::Custom("test")),
	// 		),
	// 		Err(target::MessageProofError::Custom("test")),
	// 	);
	// }
	//
	// #[test]
	// fn message_proof_is_rejected_if_required_message_is_missing() {
	// 	assert_eq!(
	// 		target::verify_messages_proof_with_parser::<OnThisChainBridge, _, _>(
	// 			messages_proof(10),
	// 			10,
	// 			|_, _| Ok(TestMessageProofParser {
	// 				failing: false,
	// 				messages: 1..=5,
	// 				outbound_lane_data: None,
	// 			}),
	// 		),
	// 		Err(target::MessageProofError::MissingRequiredMessage),
	// 	);
	// }
	//
	// #[test]
	// fn message_proof_is_rejected_if_message_decode_fails() {
	// 	assert_eq!(
	// 		target::verify_messages_proof_with_parser::<OnThisChainBridge, _, _>(
	// 			messages_proof(10),
	// 			10,
	// 			|_, _| Ok(TestMessageProofParser {
	// 				failing: true,
	// 				messages: 1..=10,
	// 				outbound_lane_data: None,
	// 			}),
	// 		),
	// 		Err(target::MessageProofError::FailedToDecodeMessage),
	// 	);
	// }
	//
	// #[test]
	// fn message_proof_is_rejected_if_outbound_lane_state_decode_fails() {
	// 	assert_eq!(
	// 		target::verify_messages_proof_with_parser::<OnThisChainBridge, _, _>(
	// 			messages_proof(0),
	// 			0,
	// 			|_, _| Ok(TestMessageProofParser {
	// 				failing: true,
	// 				messages: no_messages_range(),
	// 				outbound_lane_data: Some(OutboundLaneData {
	// 					oldest_unpruned_nonce: 1,
	// 					latest_received_nonce: 1,
	// 					latest_generated_nonce: 1,
	// 				}),
	// 			}),
	// 		),
	// 		Err(target::MessageProofError::FailedToDecodeOutboundLaneState),
	// 	);
	// }
	//
	// #[test]
	// fn message_proof_is_rejected_if_it_is_empty() {
	// 	assert_eq!(
	// 		target::verify_messages_proof_with_parser::<OnThisChainBridge, _, _>(
	// 			messages_proof(0),
	// 			0,
	// 			|_, _| Ok(TestMessageProofParser {
	// 				failing: false,
	// 				messages: no_messages_range(),
	// 				outbound_lane_data: None,
	// 			}),
	// 		),
	// 		Err(target::MessageProofError::Empty),
	// 	);
	// }
	//
	// #[test]
	// fn non_empty_message_proof_without_messages_is_accepted() {
	// 	assert_eq!(
	// 		target::verify_messages_proof_with_parser::<OnThisChainBridge, _, _>(
	// 			messages_proof(0),
	// 			0,
	// 			|_, _| Ok(TestMessageProofParser {
	// 				failing: false,
	// 				messages: no_messages_range(),
	// 				outbound_lane_data: Some(OutboundLaneData {
	// 					oldest_unpruned_nonce: 1,
	// 					latest_received_nonce: 1,
	// 					latest_generated_nonce: 1,
	// 				}),
	// 			}),
	// 		),
	// 		Ok(vec![(
	// 			Default::default(),
	// 			ProvedLaneMessages {
	// 				lane_state: Some(OutboundLaneData {
	// 					oldest_unpruned_nonce: 1,
	// 					latest_received_nonce: 1,
	// 					latest_generated_nonce: 1,
	// 				}),
	// 				messages: Vec::new(),
	// 			},
	// 		)]
	// 		.into_iter()
	// 		.collect()),
	// 	);
	// }
	//
	// #[test]
	// fn non_empty_message_proof_is_accepted() {
	// 	assert_eq!(
	// 		target::verify_messages_proof_with_parser::<OnThisChainBridge, _, _>(
	// 			messages_proof(1),
	// 			1,
	// 			|_, _| Ok(TestMessageProofParser {
	// 				failing: false,
	// 				messages: 1..=1,
	// 				outbound_lane_data: Some(OutboundLaneData {
	// 					oldest_unpruned_nonce: 1,
	// 					latest_received_nonce: 1,
	// 					latest_generated_nonce: 1,
	// 				}),
	// 			}),
	// 		),
	// 		Ok(vec![(
	// 			Default::default(),
	// 			ProvedLaneMessages {
	// 				lane_state: Some(OutboundLaneData {
	// 					oldest_unpruned_nonce: 1,
	// 					latest_received_nonce: 1,
	// 					latest_generated_nonce: 1,
	// 				}),
	// 				messages: vec![Message {
	// 					key: MessageKey { lane_id: Default::default(), nonce: 1 },
	// 					data: MessageData { payload: 1u64.encode(), fee: BridgedChainBalance(0) },
	// 				}],
	// 			},
	// 		)]
	// 		.into_iter()
	// 		.collect()),
	// 	);
	// }
	//
	// #[test]
	// fn verify_messages_proof_with_parser_does_not_panic_if_messages_count_mismatches() {
	// 	assert_eq!(
	// 		target::verify_messages_proof_with_parser::<OnThisChainBridge, _, _>(
	// 			messages_proof(u64::MAX),
	// 			0,
	// 			|_, _| Ok(TestMessageProofParser {
	// 				failing: false,
	// 				messages: 0..=u64::MAX,
	// 				outbound_lane_data: Some(OutboundLaneData {
	// 					oldest_unpruned_nonce: 1,
	// 					latest_received_nonce: 1,
	// 					latest_generated_nonce: 1,
	// 				}),
	// 			}),
	// 		),
	// 		Err(target::MessageProofError::MessagesCountMismatch),
	// 	);
	// }
}
