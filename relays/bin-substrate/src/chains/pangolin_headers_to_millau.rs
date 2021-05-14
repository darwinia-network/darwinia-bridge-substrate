use crate::finality_pipeline::{SubstrateFinalitySyncPipeline, SubstrateFinalityToSubstrate};

use bp_header_chain::justification::GrandpaJustification;
use codec::Encode;
use pangolin_runtime_params::s2s as s2s_params;
use relay_millau_client::{Millau, SigningParams as MillauSigningParams};
use relay_pangolin_client::{
	PangolinRelayChain, SigningParams as PangolinSigningParams, SyncHeader as PangolinSyncHeader,
};
use relay_substrate_client::{Chain, TransactionSignScheme};
use sp_core::{Bytes, Pair};

/// Pangolin-to-Millau finality sync pipeline.
pub(crate) type PangolinFinalityToMillau =
	SubstrateFinalityToSubstrate<PangolinRelayChain, Millau, PangolinSigningParams>;

impl SubstrateFinalitySyncPipeline for PangolinFinalityToMillau {
	const BEST_FINALIZED_SOURCE_HEADER_ID_AT_TARGET: &'static str = s2s_params::BEST_FINALIZED_PANGOLIN_HEADER_METHOD;

	type TargetChain = Millau;

	fn transactions_author(&self) -> bp_millau::AccountId {
		(*self.target_sign.public().as_array_ref()).into()
	}

	fn make_submit_finality_proof_transaction(
		&self,
		transaction_nonce: <Millau as Chain>::Index,
		header: PangolinSyncHeader,
		proof: GrandpaJustification<drml_primitives::Header>,
	) -> Bytes {
		let call = millau_runtime::BridgeGrandpaPangolinCall::<
			millau_runtime::Runtime,
			millau_runtime::WithPangolinGrandpaInstance,
		>::submit_finality_proof(header.into_inner(), proof)
		.into();

		let genesis_hash = *self.target_client.genesis_hash();
		let transaction = Millau::sign_transaction(genesis_hash, &self.target_sign, transaction_nonce, call);

		Bytes(transaction.encode())
	}
}