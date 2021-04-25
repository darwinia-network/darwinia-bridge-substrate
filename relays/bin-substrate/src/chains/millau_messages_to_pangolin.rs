use crate::messages_lane::{
	select_delivery_transaction_limits, MessagesRelayParams, SubstrateMessageLane, SubstrateMessageLaneToSubstrate,
};
use crate::messages_source::SubstrateMessagesSource;
use crate::messages_target::SubstrateMessagesTarget;

use bp_messages::MessageNonce;
use bp_runtime::{MILLAU_BRIDGE_INSTANCE, PANGOLIN_BRIDGE_INSTANCE};
use bridge_runtime_common::messages::target::FromBridgedChainMessagesProof;
use codec::Encode;
use frame_support::dispatch::GetDispatchInfo;
use messages_relay::message_lane::MessageLane;
use relay_millau_client::{HeaderId as MillauHeaderId, Millau, SigningParams as MillauSigningParams};
use pangolin_runtime::{
	HeaderId as PangolinHeaderId,
	PangolinRelayChain,
	SigningParams as PangolinSigningParams,
};
use relay_substrate_client::{
	metrics::{FloatStorageValueMetric, StorageProofOverheadMetric},
	Chain, TransactionSignScheme,
};
use sp_core::{Bytes, Pair};
use std::{ops::RangeInclusive, time::Duration};


/// Millau-to-Pangolin message lane.
pub type MillauMessagesToPangolin = SubstrateMessageLaneToSubstrate<
	Millau,
	MillauSigningParams,
	PangolinRelayChain,
	PangolinSigningParams
>;


impl SubstrateMessageLane for MillauMessagesToPangolin {
	const OUTBOUND_LANE_MESSAGES_DISPATCH_WEIGHT_METHOD: &'static str =
		pangolin_runtime::TO_PANGOLIN_MESSAGES_DISPATCH_WEIGHT_METHOD;
	const OUTBOUND_LANE_LATEST_GENERATED_NONCE_METHOD: &'static str =
		pangolin_runtime::TO_PANGOLIN_LATEST_GENERATED_NONCE_METHOD;
	const OUTBOUND_LANE_LATEST_RECEIVED_NONCE_METHOD: &'static str =
		pangolin_runtime::TO_PANGOLIN_LATEST_RECEIVED_NONCE_METHOD;

	const INBOUND_LANE_LATEST_RECEIVED_NONCE_METHOD: &'static str =
		bp_millau::FROM_MILLAU_LATEST_RECEIVED_NONCE_METHOD;
	const INBOUND_LANE_LATEST_CONFIRMED_NONCE_METHOD: &'static str =
		bp_millau::FROM_MILLAU_LATEST_CONFIRMED_NONCE_METHOD;
	const INBOUND_LANE_UNREWARDED_RELAYERS_STATE: &'static str =
		bp_millau::FROM_MILLAU_UNREWARDED_RELAYERS_STATE;

	const BEST_FINALIZED_SOURCE_HEADER_ID_AT_TARGET: &'static str =
		bp_millau::BEST_FINALIZED_MILLAU_HEADER_METHOD;
	const BEST_FINALIZED_TARGET_HEADER_ID_AT_SOURCE: &'static str =
		pangolin_runtime::BEST_FINALIZED_PANGOLIN_HEADER_METHOD;

	type SourceChain = Millau;
	type TargetChain = PangolinRelayChain;

	fn source_transactions_author(&self) -> pangolin_runtime::AccountId {
		(*self.source_sign.public().as_array_ref()).into()
	}

	fn make_messages_receiving_proof_transaction(
		&self,
		transaction_nonce: <Millau as Chain>::Index,
		_generated_at_block: PangolinHeaderId,
		proof: <Self as MessageLane>::MessagesReceivingProof,
	) -> Bytes {
		let (relayers_state, proof) = proof;
		let call: millau_runtime::Call =
			millau_runtime::MessagesCall::receive_messages_delivery_proof(proof, relayers_state).into();
		let call_weight = call.get_dispatch_info().weight;
		let genesis_hash = *self.source_client.genesis_hash();
		let transaction = Millau::sign_transaction(
			genesis_hash,
			&self.source_sign,
			transaction_nonce,
			call,
		);
		log::trace!(
			target: "bridge",
			"Prepared Pangolin -> Millau confirmation transaction. Weight: {}/{}, size: {}/{}",
			call_weight,
			bp_millau::max_extrinsic_weight(),
			transaction.encode().len(),
			bp_millau::max_extrinsic_size(),
		);
		Bytes(transaction.encode())
	}

	fn target_transactions_author(&self) -> pangolin_runtime::AccountId {
		(*self.target_sign.public().as_array_ref()).into()
	}


	fn make_messages_delivery_transaction(
		&self,
		transaction_nonce: <Pangolin as Chain>::Index,
		_generated_at_header: MillauHeaderId,
		_nonces: RangeInclusive<MessageNonce>,
		proof: <Self as MessageLane>::MessagesProof,
	) -> Bytes {
		let (dispatch_weight, proof) = proof;
		let FromBridgedChainMessagesProof {
			ref nonces_start,
			ref nonces_end,
			..
		} = proof;
		let messages_count = nonces_end - nonces_start + 1;
		let call: pangolin_runtime::Call = pangolin_runtime::MessagesCall::receive_messages_proof(
			self.relayer_id_at_source.clone(),
			proof,
			messages_count as _,
			dispatch_weight,
		)
			.into();
		let call_weight = call.get_dispatch_info().weight;
		let genesis_hash = *self.target_client.genesis_hash();
		let transaction = PangolinRelayChain::sign_transaction(
			genesis_hash,
			&self.target_sign,
			transaction_nonce,
			call,
		);
		log::trace!(
			target: "bridge",
			"Prepared Millau -> Pangolin delivery transaction. Weight: {}/{}, size: {}/{}",
			call_weight,
			pangolin_runtime::max_extrinsic_weight(),
			transaction.encode().len(),
			pangolin_runtime::max_extrinsic_size(),
		);
		Bytes(transaction.encode())
	}
}


/// Millau node as messages source.
type MillauSourceClient = SubstrateMessagesSource<
	Millau,
	MillauMessagesToPangolin,
	millau_runtime::Runtime,
	millau_runtime::WithPangolinMessagesInstance,
>;

/// Pangolin node as messages target.
type PangolinTargetClient = SubstrateMessagesTarget<
	PangolinRelayChain,
	MillauMessagesToPangolin,
	pangolin_runtime::Runtime,
	pangolin_runtime::WithMillauMessagesInstance,
>;


/// Run Millau-to-Pangolin messages sync.
pub async fn run(
	params: MessagesRelayParams<Millau, MillauSigningParams, PangolinRelayChain, PangolinSigningParams>,
) -> Result<(), String> {
	let stall_timeout = Duration::from_secs(5 * 60);
	let relayer_id_at_millau = (*params.source_sign.public().as_array_ref()).into();

	let lane_id = params.lane_id;
	let source_client = params.source_client;
	let lane = MillauMessagesToPangolin {
		source_client: source_client.clone(),
		source_sign: params.source_sign,
		target_client: params.target_client.clone(),
		target_sign: params.target_sign,
		relayer_id_at_source: relayer_id_at_millau,
	};

	// 2/3 is reserved for proofs and tx overhead
	let max_messages_size_in_single_batch = pangolin_runtime::max_extrinsic_size() as usize / 3;
	// TODO: use Millau weights after https://github.com/paritytech/parity-bridges-common/issues/390
	let (max_messages_in_single_batch, max_messages_weight_in_single_batch) =
		select_delivery_transaction_limits::<
			pallet_bridge_messages::weights::RialtoWeight<millau_runtime::Runtime>
		>(
			pangolin_runtime::max_extrinsic_weight(),
			pangolin_runtime::MAX_UNREWARDED_RELAYER_ENTRIES_AT_INBOUND_LANE,
		);

	log::info!(
		target: "bridge",
		"Starting Millau -> Pangolin messages relay.\n\t\
			Millau relayer account id: {:?}\n\t\
			Max messages in single transaction: {}\n\t\
			Max messages size in single transaction: {}\n\t\
			Max messages weight in single transaction: {}",
		lane.relayer_id_at_source,
		max_messages_in_single_batch,
		max_messages_size_in_single_batch,
		max_messages_weight_in_single_batch,
	);

	messages_relay::message_lane_loop::run(
		messages_relay::message_lane_loop::Params {
			lane: lane_id,
			source_tick: Millau::AVERAGE_BLOCK_INTERVAL,
			target_tick: PangolinRelayChain::AVERAGE_BLOCK_INTERVAL,
			reconnect_delay: relay_utils::relay_loop::RECONNECT_DELAY,
			stall_timeout,
			delivery_params: messages_relay::message_lane_loop::MessageDeliveryParams {
				max_unrewarded_relayer_entries_at_target: pangolin_runtime::MAX_UNREWARDED_RELAYER_ENTRIES_AT_INBOUND_LANE,
				max_unconfirmed_nonces_at_target: pangolin_runtime::MAX_UNCONFIRMED_MESSAGES_AT_INBOUND_LANE,
				max_messages_in_single_batch,
				max_messages_weight_in_single_batch,
				max_messages_size_in_single_batch,
			},
		},
		MillauSourceClient::new(
			source_client.clone(),
			lane.clone(),
			lane_id,
			PANGOLIN_BRIDGE_INSTANCE,
			params.target_to_source_headers_relay,
		),
		PangolinTargetClient::new(
			params.target_client,
			lane,
			lane_id,
			MILLAU_BRIDGE_INSTANCE,
			params.source_to_target_headers_relay,
		),
		relay_utils::relay_metrics(
			Some(messages_relay::message_lane_loop::metrics_prefix::<
				MillauMessagesToPangolin,
			>(&lane_id)),
			params.metrics_params,
		)
			.standalone_metric(|registry, prefix| {
				StorageProofOverheadMetric::new(
					registry,
					prefix,
					source_client.clone(),
					"millau_storage_proof_overhead".into(),
					"Millau storage proof overhead".into(),
				)
			})?
			.standalone_metric(|registry, prefix| {
				FloatStorageValueMetric::<_, sp_runtime::FixedU128>::new(
					registry,
					prefix,
					source_client,
					sp_core::storage::StorageKey(
						millau_runtime::rialto_messages::RialtoToMillauConversionRate::key().to_vec(),
					),
					Some(millau_runtime::rialto_messages::INITIAL_RIALTO_TO_MILLAU_CONVERSION_RATE),
					"millau_rialto_to_millau_conversion_rate".into(),
					"Rialto to Millau tokens conversion rate (used by Rialto)".into(),
				)
			})?
			.into_params(),
		futures::future::pending(),
	)
		.await
}
