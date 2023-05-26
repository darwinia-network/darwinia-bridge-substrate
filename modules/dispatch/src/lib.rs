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

//! Runtime module which takes care of dispatching messages received over the bridge.
//!
//! The messages are interpreted directly as runtime `Call`. We attempt to decode
//! them and then dispatch as usual. To prevent compatibility issues, the Calls have
//! to include a `spec_version`. This will be checked before dispatch. In the case of
//! a successful dispatch an event is emitted.

#![cfg_attr(not(feature = "std"), no_std)]
// Generated by `decl_event!`
#![allow(clippy::unused_unit)]

// crates.io
use codec::Encode;
// darwinia-network
use bp_message_dispatch::{
	CallOrigin, CallValidate, IntoDispatchOrigin, MessageDispatch, MessagePayload, SpecVersion,
};
use bp_runtime::{
	derive_account_id,
	messages::{DispatchFeePayment, MessageDispatchResult},
	ChainId, SourceAccount,
};
// substrate
use frame_support::{
	dispatch::{DispatchInfo, DispatchResultWithPostInfo, Dispatchable, GetDispatchInfo, Weight},
	ensure, log,
	pallet_prelude::Pays,
	traits::Get,
};
use frame_system::RawOrigin;
use sp_runtime::traits::{BadOrigin, Convert, IdentifyAccount, MaybeDisplay, Verify};
use sp_std::{fmt::Debug, prelude::*};

#[frame_support::pallet]
pub mod pallet {
	// darwinia-network
	use super::*;
	// substrate
	use frame_support::pallet_prelude::*;
	use frame_system::pallet_prelude::*;

	#[pallet::config]
	pub trait Config<I: 'static = ()>: frame_system::Config {
		/// The overarching event type.
		type RuntimeEvent: From<Event<Self, I>>
			+ IsType<<Self as frame_system::Config>::RuntimeEvent>;
		/// Id of the message. Whenever message is passed to the dispatch module, it emits
		/// event with this id + dispatch result. Could be e.g. (LaneId, MessageNonce) if
		/// it comes from the messages module.
		type BridgeMessageId: Parameter;
		/// Type of account ID on source chain.
		type SourceChainAccountId: Parameter
			+ Member
			+ MaybeSerializeDeserialize
			+ Debug
			+ MaybeDisplay
			+ Ord;
		/// Type of account public key on target chain.
		type TargetChainAccountPublic: Parameter + IdentifyAccount<AccountId = Self::AccountId>;
		/// Type of signature that may prove that the message has been signed by
		/// owner of `TargetChainAccountPublic`.
		type TargetChainSignature: Parameter + Verify<Signer = Self::TargetChainAccountPublic>;
		/// The overarching dispatch call type.
		type RuntimeCall: Parameter
			+ GetDispatchInfo
			+ Dispatchable<
				RuntimeOrigin = <Self as frame_system::Config>::RuntimeOrigin,
				PostInfo = frame_support::dispatch::PostDispatchInfo,
			>;
		/// Pre-dispatch validation for incoming calls.
		///
		/// The pallet will validate all incoming calls right before they're dispatched. If this
		/// validator rejects the call, special event (`Event::MessageCallRejected`) is emitted.
		type CallValidator: CallValidate<
			Self::AccountId,
			Self::RuntimeOrigin,
			<Self as Config<I>>::RuntimeCall,
		>;
		/// The type that is used to wrap the `Self::Call` when it is moved over bridge.
		///
		/// The idea behind this is to avoid `Call` conversion/decoding until we'll be sure
		/// that all other stuff (like `spec_version`) is ok. If we would try to decode
		/// `Call` which has been encoded using previous `spec_version`, then we might end
		/// up with decoding error, instead of `MessageVersionSpecMismatch`.
		type EncodedCall: Decode
			+ Encode
			+ Into<Result<<Self as Config<I>>::RuntimeCall, ()>>
			+ Clone;
		/// A type which can be turned into an AccountId from a 256-bit hash.
		///
		/// Used when deriving target chain AccountIds from source chain AccountIds.
		type AccountIdConverter: sp_runtime::traits::Convert<sp_core::hash::H256, Self::AccountId>;
		/// The type is used to customize the dispatch call origin.
		type IntoDispatchOrigin: IntoDispatchOrigin<
			Self::AccountId,
			<Self as Config<I>>::RuntimeCall,
			Self::RuntimeOrigin,
		>;
	}

	type BridgeMessageIdOf<T, I> = <T as Config<I>>::BridgeMessageId;

	#[pallet::pallet]
	pub struct Pallet<T, I = ()>(PhantomData<(T, I)>);

	#[pallet::hooks]
	impl<T: Config<I>, I: 'static> Hooks<BlockNumberFor<T>> for Pallet<T, I> {}

	#[pallet::call]
	impl<T: Config<I>, I: 'static> Pallet<T, I> {}

	#[pallet::event]
	#[pallet::generate_deposit(pub(super) fn deposit_event)]
	pub enum Event<T: Config<I>, I: 'static = ()> {
		/// Message has been rejected before reaching dispatch.
		MessageRejected(ChainId, BridgeMessageIdOf<T, I>),
		/// Message has been rejected by dispatcher because of spec version mismatch.
		/// Last two arguments are: expected and passed spec version.
		MessageVersionSpecMismatch(ChainId, BridgeMessageIdOf<T, I>, SpecVersion, SpecVersion),
		/// Message has been rejected by dispatcher because of weight mismatch.
		/// Last two arguments are: expected and passed call weight.
		MessageWeightMismatch(ChainId, BridgeMessageIdOf<T, I>, Weight, Weight),
		/// Message signature mismatch.
		MessageSignatureMismatch(ChainId, BridgeMessageIdOf<T, I>),
		/// We have failed to decode Call from the message.
		MessageCallDecodeFailed(ChainId, BridgeMessageIdOf<T, I>),
		/// The call from the message has been rejected by the call validator.
		MessageCallValidateFailed(ChainId, BridgeMessageIdOf<T, I>, TransactionValidityError),
		/// The origin account has failed to pay fee for dispatching the message.
		MessageDispatchPaymentFailed(
			ChainId,
			BridgeMessageIdOf<T, I>,
			<T as frame_system::Config>::AccountId,
			Weight,
		),
		/// Message has been dispatched with given result.
		MessageDispatched(ChainId, BridgeMessageIdOf<T, I>, DispatchResult),
		/// Phantom member, never used. Needed to handle multiple pallet instances.
		_Dummy(PhantomData<I>),
	}
}
pub use pallet::*;

impl<T: Config<I>, I: 'static> MessageDispatch<T::AccountId, T::BridgeMessageId> for Pallet<T, I> {
	type Message = MessagePayload<
		T::SourceChainAccountId,
		T::TargetChainAccountPublic,
		T::TargetChainSignature,
		T::EncodedCall,
	>;

	fn dispatch_weight(message: &Self::Message) -> bp_message_dispatch::Weight {
		message.weight
	}

	fn pre_dispatch(
		relayer_account: &T::AccountId,
		message: Result<&Self::Message, ()>,
	) -> Result<(), &'static str> {
		match message {
			Ok(raw_message) =>
				if let Ok(call) = raw_message.clone().call.into() {
					return T::CallValidator::check_receiving_before_dispatch(
						relayer_account,
						&call,
					);
				},
			Err(_) => {
				log::trace!(
					target: "runtime::bridge-dispatch",
					"Message will be rejected in dispatch, still Ok here",
				);
			},
		}

		Ok(())
	}

	fn dispatch<P: FnOnce(&T::AccountId, bp_message_dispatch::Weight) -> Result<(), ()>>(
		source_chain: ChainId,
		target_chain: ChainId,
		relayer_account: &T::AccountId,
		id: T::BridgeMessageId,
		message: Result<Self::Message, ()>,
		pay_dispatch_fee: P,
	) -> MessageDispatchResult {
		// emit special even if message has been rejected by external component
		let message = match message {
			Ok(message) => message,
			Err(_) => {
				log::trace!(
					target: "runtime::bridge-dispatch",
					"Message {:?}/{:?}: rejected before actual dispatch",
					source_chain,
					id,
				);
				Self::deposit_event(Event::MessageRejected(source_chain, id));
				return MessageDispatchResult {
					dispatch_result: false,
					unspent_weight: Weight::zero(),
					dispatch_fee_paid_during_dispatch: false,
				};
			},
		};

		// verify spec version
		// (we want it to be the same, because otherwise we may decode Call improperly)
		let mut dispatch_result = MessageDispatchResult {
			dispatch_result: false,
			unspent_weight: message.weight,
			dispatch_fee_paid_during_dispatch: false,
		};
		let expected_version = <T as frame_system::Config>::Version::get().spec_version;
		if message.spec_version != expected_version {
			log::trace!(
				"Message {:?}/{:?}: spec_version mismatch. Expected {:?}, got {:?}",
				source_chain,
				id,
				expected_version,
				message.spec_version,
			);
			Self::deposit_event(Event::MessageVersionSpecMismatch(
				source_chain,
				id,
				expected_version,
				message.spec_version,
			));
			return dispatch_result;
		}

		// now that we have spec version checked, let's decode the call
		let call = match message.call.into() {
			Ok(call) => call,
			Err(_) => {
				log::trace!(
					target: "runtime::bridge-dispatch",
					"Failed to decode Call from message {:?}/{:?}",
					source_chain,
					id,
				);
				Self::deposit_event(Event::MessageCallDecodeFailed(source_chain, id));
				return dispatch_result;
			},
		};

		// prepare dispatch origin
		let origin_derived_account = match message.origin {
			CallOrigin::SourceRoot => {
				let hex_id =
					derive_account_id::<T::SourceChainAccountId>(source_chain, SourceAccount::Root);
				let target_id = T::AccountIdConverter::convert(hex_id);
				log::trace!(target: "runtime::bridge-dispatch", "Root Account: {:?}", &target_id);
				target_id
			},
			CallOrigin::TargetAccount(source_account_id, target_public, target_signature) => {
				let digest = account_ownership_digest(
					&call,
					source_account_id,
					message.spec_version,
					source_chain,
					target_chain,
				);

				let target_account = target_public.into_account();
				if !target_signature.verify(&digest[..], &target_account) {
					log::trace!(
						target: "runtime::bridge-dispatch",
						"Message {:?}/{:?}: origin proof is invalid. Expected account: {:?} from signature: {:?}",
						source_chain,
						id,
						target_account,
						target_signature,
					);
					Self::deposit_event(Event::MessageSignatureMismatch(source_chain, id));
					return dispatch_result;
				}

				log::trace!(target: "runtime::bridge-dispatch", "Target Account: {:?}", &target_account);
				target_account
			},
			CallOrigin::SourceAccount(source_account_id) => {
				let hex_id =
					derive_account_id(source_chain, SourceAccount::Account(source_account_id));
				let target_id = T::AccountIdConverter::convert(hex_id);
				log::trace!(target: "runtime::bridge-dispatch", "Source Account: {:?}", &target_id);
				target_id
			},
		};

		// generate dispatch origin from origin account
		let dispatch_origin =
			T::IntoDispatchOrigin::into_dispatch_origin(&origin_derived_account, &call);

		// validate the call
		if let Err(e) = T::CallValidator::call_validate(relayer_account, &dispatch_origin, &call) {
			log::trace!(
				target: "runtime::bridge-dispatch",
				"Message {:?}/{:?}: the call ({:?}) is rejected by the validator",
				source_chain,
				id,
				call,
			);
			Self::deposit_event(Event::MessageCallValidateFailed(source_chain, id, e));
			return dispatch_result;
		}

		// verify weight
		// (we want passed weight to be at least equal to pre-dispatch weight of the call
		// because otherwise Calls may be dispatched at lower price)
		let dispatch_info = call.get_dispatch_info();
		let expected_weight = dispatch_info.weight;
		if message.weight.ref_time() < expected_weight.ref_time() {
			log::trace!(
				target: "runtime::bridge-dispatch",
				"Message {:?}/{:?}: passed weight is too low. Expected at least {:?}, got {:?}",
				source_chain,
				id,
				expected_weight,
				message.weight,
			);
			Self::deposit_event(Event::MessageWeightMismatch(
				source_chain,
				id,
				expected_weight,
				message.weight,
			));
			return dispatch_result;
		}

		// pay dispatch fee right before dispatch
		let pay_dispatch_fee_at_target_chain =
			message.dispatch_fee_payment == DispatchFeePayment::AtTargetChain;
		if pay_dispatch_fee_at_target_chain
			&& pay_dispatch_fee(&origin_derived_account, message.weight).is_err()
		{
			log::trace!(
				target: "runtime::bridge-dispatch",
				"Failed to pay dispatch fee for dispatching message {:?}/{:?} with weight {}",
				source_chain,
				id,
				message.weight,
			);
			Self::deposit_event(Event::MessageDispatchPaymentFailed(
				source_chain,
				id,
				origin_derived_account,
				message.weight,
			));
			return dispatch_result;
		}
		dispatch_result.dispatch_fee_paid_during_dispatch = pay_dispatch_fee_at_target_chain;

		log::trace!(target: "runtime::bridge-dispatch", "Message being dispatched is: {:.4096?}", &call);
		let result = call.dispatch(dispatch_origin);
		let actual_call_weight = extract_actual_weight(&result, &dispatch_info);
		dispatch_result.dispatch_result = result.is_ok();
		dispatch_result.unspent_weight = message.weight.saturating_sub(actual_call_weight);

		log::trace!(
			target: "runtime::bridge-dispatch",
			"Message {:?}/{:?} has been dispatched. Weight: {} of {}. Result: {:?}. Call dispatch result: {:?}",
			source_chain,
			id,
			actual_call_weight,
			message.weight,
			dispatch_result,
			result,
		);

		Self::deposit_event(Event::MessageDispatched(
			source_chain,
			id,
			result.map(drop).map_err(|e| e.error),
		));

		dispatch_result
	}
}

fn extract_actual_weight(result: &DispatchResultWithPostInfo, info: &DispatchInfo) -> Weight {
	let post_info = match result {
		Ok(post_info) => post_info,
		Err(err) => &err.post_info,
	};
	match post_info.pays_fee {
		Pays::Yes => post_info.calc_actual_weight(info),
		Pays::No => Weight::zero(),
	}
}

/// Check if the message is allowed to be dispatched on the target chain given the sender's origin
/// on the source chain.
///
/// For example, if a message is sent from a "regular" account on the source chain it will not be
/// allowed to be dispatched as Root on the target chain. This is a useful check to do on the source
/// chain _before_ sending a message whose dispatch will be rejected on the target chain.
pub fn verify_message_origin<
	SourceChainAccountId,
	TargetChainAccountPublic,
	TargetChainSignature,
	Call,
>(
	sender_origin: &RawOrigin<SourceChainAccountId>,
	message: &MessagePayload<
		SourceChainAccountId,
		TargetChainAccountPublic,
		TargetChainSignature,
		Call,
	>,
) -> Result<Option<SourceChainAccountId>, BadOrigin>
where
	SourceChainAccountId: PartialEq + Clone,
{
	match message.origin {
		CallOrigin::SourceRoot => {
			ensure!(sender_origin == &RawOrigin::Root, BadOrigin);
			Ok(None)
		},
		CallOrigin::TargetAccount(ref source_account_id, _, _) => {
			ensure!(sender_origin == &RawOrigin::Signed(source_account_id.clone()), BadOrigin);
			Ok(Some(source_account_id.clone()))
		},
		CallOrigin::SourceAccount(ref source_account_id) => {
			ensure!(
				sender_origin == &RawOrigin::Signed(source_account_id.clone())
					|| sender_origin == &RawOrigin::Root,
				BadOrigin
			);
			Ok(Some(source_account_id.clone()))
		},
	}
}

/// Target account ownership digest from the source chain.
///
/// The byte vector returned by this function will be signed with a target chain account
/// private key. This way, the owner of `source_account_id` on the source chain proves that
/// the target chain account private key is also under his control.
pub fn account_ownership_digest<Call, AccountId, SpecVersion>(
	call: &Call,
	source_account_id: AccountId,
	target_spec_version: SpecVersion,
	source_chain_id: ChainId,
	target_chain_id: ChainId,
) -> Vec<u8>
where
	Call: Encode,
	AccountId: Encode,
	SpecVersion: Encode,
{
	let mut proof = Vec::new();
	call.encode_to(&mut proof);
	source_account_id.encode_to(&mut proof);
	target_spec_version.encode_to(&mut proof);
	source_chain_id.encode_to(&mut proof);
	target_chain_id.encode_to(&mut proof);

	proof
}

#[cfg(test)]
mod tests {
	// From construct_runtime macro
	#![allow(clippy::from_over_into)]

	// crates.io
	use codec::Decode;
	// darwinia-network
	use super::*;
	use crate as call_dispatch;
	// substrate
	use frame_support::{parameter_types, weights::Weight};
	use frame_system::{mocking::*, EventRecord, Phase};
	use scale_info::TypeInfo;
	use sp_core::H256;
	use sp_runtime::{
		testing::Header,
		traits::{BlakeTwo256, IdentityLookup},
		transaction_validity::{InvalidTransaction, TransactionValidityError},
		Perbill,
	};

	type AccountId = u64;
	type BridgeMessageId = [u8; 4];

	type Block = MockBlock<TestRuntime>;
	type UncheckedExtrinsic = MockUncheckedExtrinsic<TestRuntime>;

	const SOURCE_CHAIN_ID: ChainId = *b"srce";
	const TARGET_CHAIN_ID: ChainId = *b"trgt";

	const TEST_SPEC_VERSION: SpecVersion = 0;
	const TEST_WEIGHT: Weight = Weight::from_parts(1_000_000_000, 0);

	#[derive(Debug, Encode, Decode, Clone, PartialEq, Eq, TypeInfo)]
	pub struct TestAccountPublic(AccountId);
	impl IdentifyAccount for TestAccountPublic {
		type AccountId = AccountId;

		fn into_account(self) -> AccountId {
			self.0
		}
	}

	#[derive(Debug, Encode, Decode, Clone, PartialEq, Eq, TypeInfo)]
	pub struct TestSignature(AccountId);
	impl Verify for TestSignature {
		type Signer = TestAccountPublic;

		fn verify<L: sp_runtime::traits::Lazy<[u8]>>(&self, _msg: L, signer: &AccountId) -> bool {
			self.0 == *signer
		}
	}

	pub struct AccountIdConverter;
	impl sp_runtime::traits::Convert<H256, AccountId> for AccountIdConverter {
		fn convert(hash: H256) -> AccountId {
			hash.to_low_u64_ne()
		}
	}

	frame_support::construct_runtime! {
		pub enum TestRuntime where
			Block = Block,
			NodeBlock = Block,
			UncheckedExtrinsic = UncheckedExtrinsic,
		{
			RootTesting: pallet_root_testing,
			System: frame_system::{Pallet, Call, Config, Storage, Event<T>},
			Dispatch: call_dispatch::{Pallet, Call, Event<T>},
		}
	}

	parameter_types! {
		pub const BlockHashCount: u64 = 250;
		pub const MaximumBlockWeight: Weight = Weight::from_parts(1024, 0);
		pub const MaximumBlockLength: u32 = 2 * 1024;
		pub const AvailableBlockRatio: Perbill = Perbill::one();
	}

	impl pallet_root_testing::Config for TestRuntime {}

	impl frame_system::Config for TestRuntime {
		type AccountData = ();
		type AccountId = AccountId;
		type BaseCallFilter = frame_support::traits::Everything;
		type BlockHashCount = BlockHashCount;
		type BlockLength = ();
		type BlockNumber = u64;
		type BlockWeights = ();
		type DbWeight = ();
		type Hash = H256;
		type Hashing = BlakeTwo256;
		type Header = Header;
		type Index = u64;
		type Lookup = IdentityLookup<Self::AccountId>;
		type MaxConsumers = frame_support::traits::ConstU32<16>;
		type OnKilledAccount = ();
		type OnNewAccount = ();
		type OnSetCode = ();
		type PalletInfo = PalletInfo;
		type RuntimeCall = RuntimeCall;
		type RuntimeEvent = RuntimeEvent;
		type RuntimeOrigin = RuntimeOrigin;
		type SS58Prefix = ();
		type SystemWeightInfo = ();
		type Version = ();
	}

	impl Config for TestRuntime {
		type AccountIdConverter = AccountIdConverter;
		type BridgeMessageId = BridgeMessageId;
		type CallValidator = CallValidator;
		type EncodedCall = EncodedCall;
		type IntoDispatchOrigin = TestIntoDispatchOrigin;
		type RuntimeCall = RuntimeCall;
		type RuntimeEvent = RuntimeEvent;
		type SourceChainAccountId = AccountId;
		type TargetChainAccountPublic = TestAccountPublic;
		type TargetChainSignature = TestSignature;
	}

	#[derive(Decode, Encode, Clone)]
	pub struct EncodedCall(Vec<u8>);

	impl From<EncodedCall> for Result<RuntimeCall, ()> {
		fn from(call: EncodedCall) -> Result<RuntimeCall, ()> {
			RuntimeCall::decode(&mut &call.0[..]).map_err(drop)
		}
	}

	pub struct CallValidator;
	impl CallValidate<AccountId, RuntimeOrigin, RuntimeCall> for CallValidator {
		fn check_receiving_before_dispatch(
			_relayer_account: &AccountId,
			_call: &RuntimeCall,
		) -> Result<(), &'static str> {
			Ok(())
		}

		fn call_validate(
			_relayer_account: &AccountId,
			_origin: &RuntimeOrigin,
			call: &RuntimeCall,
		) -> Result<(), TransactionValidityError> {
			match call {
				RuntimeCall::RootTesting(pallet_root_testing::Call::fill_block { .. }) =>
					Err(TransactionValidityError::Invalid(InvalidTransaction::Call)),
				_ => Ok(()),
			}
		}
	}

	pub struct TestIntoDispatchOrigin;
	impl IntoDispatchOrigin<AccountId, RuntimeCall, RuntimeOrigin> for TestIntoDispatchOrigin {
		fn into_dispatch_origin(id: &AccountId, _call: &RuntimeCall) -> RuntimeOrigin {
			frame_system::RawOrigin::Signed(*id).into()
		}
	}

	fn new_test_ext() -> sp_io::TestExternalities {
		let t = frame_system::GenesisConfig::default().build_storage::<TestRuntime>().unwrap();
		sp_io::TestExternalities::new(t)
	}

	fn prepare_message(
		origin: CallOrigin<AccountId, TestAccountPublic, TestSignature>,
		call: RuntimeCall,
	) -> <Pallet<TestRuntime> as MessageDispatch<
		AccountId,
		<TestRuntime as Config>::BridgeMessageId,
	>>::Message {
		MessagePayload {
			spec_version: TEST_SPEC_VERSION,
			weight: TEST_WEIGHT,
			origin,
			dispatch_fee_payment: DispatchFeePayment::AtSourceChain,
			call: EncodedCall(call.encode()),
		}
	}

	fn prepare_root_message(
		call: RuntimeCall,
	) -> <Pallet<TestRuntime> as MessageDispatch<
		AccountId,
		<TestRuntime as Config>::BridgeMessageId,
	>>::Message {
		prepare_message(CallOrigin::SourceRoot, call)
	}

	fn prepare_target_message(
		call: RuntimeCall,
	) -> <Pallet<TestRuntime> as MessageDispatch<
		AccountId,
		<TestRuntime as Config>::BridgeMessageId,
	>>::Message {
		let origin = CallOrigin::TargetAccount(1, TestAccountPublic(1), TestSignature(1));
		prepare_message(origin, call)
	}

	fn prepare_source_message(
		call: RuntimeCall,
	) -> <Pallet<TestRuntime> as MessageDispatch<
		AccountId,
		<TestRuntime as Config>::BridgeMessageId,
	>>::Message {
		let origin = CallOrigin::SourceAccount(1);
		prepare_message(origin, call)
	}

	#[test]
	fn should_fail_on_spec_version_mismatch() {
		new_test_ext().execute_with(|| {
			let id = [0; 4];
			let relayer_account = 1;

			const BAD_SPEC_VERSION: SpecVersion = 99;
			let mut message =
				prepare_root_message(RuntimeCall::System(frame_system::Call::remark {
					remark: vec![1, 2, 3],
				}));
			let weight = message.weight;
			message.spec_version = BAD_SPEC_VERSION;

			System::set_block_number(1);
			let result = Dispatch::dispatch(
				SOURCE_CHAIN_ID,
				TARGET_CHAIN_ID,
				&relayer_account,
				id,
				Ok(message),
				|_, _| unreachable!(),
			);
			assert_eq!(result.unspent_weight, weight);
			assert!(!result.dispatch_result);

			assert_eq!(
				System::events(),
				vec![EventRecord {
					phase: Phase::Initialization,
					event: RuntimeEvent::Dispatch(
						call_dispatch::Event::<TestRuntime>::MessageVersionSpecMismatch(
							SOURCE_CHAIN_ID,
							id,
							TEST_SPEC_VERSION,
							BAD_SPEC_VERSION
						)
					),
					topics: vec![],
				}],
			);
		});
	}

	#[test]
	fn should_fail_on_weight_mismatch() {
		new_test_ext().execute_with(|| {
			let id = [0; 4];
			let relayer_account = 1;
			let call = RuntimeCall::System(frame_system::Call::set_heap_pages { pages: 42 });
			let call_weight = call.get_dispatch_info().weight;
			let mut message = prepare_root_message(call);
			message.weight = Weight::from_parts(7, 0);
			assert!(
				call_weight.ref_time() > message.weight.ref_time(),
				"needed for test to actually trigger a weight mismatch"
			);

			System::set_block_number(1);
			let result = Dispatch::dispatch(
				SOURCE_CHAIN_ID,
				TARGET_CHAIN_ID,
				&relayer_account,
				id,
				Ok(message),
				|_, _| unreachable!(),
			);
			assert_eq!(result.unspent_weight, Weight::from_parts(7, 0));
			assert!(!result.dispatch_result);

			assert_eq!(
				System::events(),
				vec![EventRecord {
					phase: Phase::Initialization,
					event: RuntimeEvent::Dispatch(
						call_dispatch::Event::<TestRuntime>::MessageWeightMismatch(
							SOURCE_CHAIN_ID,
							id,
							call_weight,
							Weight::from_parts(7, 0),
						)
					),
					topics: vec![],
				}],
			);
		});
	}

	#[test]
	fn should_fail_on_signature_mismatch() {
		new_test_ext().execute_with(|| {
			let id = [0; 4];
			let relayer_account = 1;

			let call_origin = CallOrigin::TargetAccount(1, TestAccountPublic(1), TestSignature(99));
			let message = prepare_message(
				call_origin,
				RuntimeCall::System(frame_system::Call::remark { remark: vec![1, 2, 3] }),
			);
			let weight = message.weight;

			System::set_block_number(1);
			let result = Dispatch::dispatch(
				SOURCE_CHAIN_ID,
				TARGET_CHAIN_ID,
				&relayer_account,
				id,
				Ok(message),
				|_, _| unreachable!(),
			);
			assert_eq!(result.unspent_weight, weight);
			assert!(!result.dispatch_result);

			assert_eq!(
				System::events(),
				vec![EventRecord {
					phase: Phase::Initialization,
					event: RuntimeEvent::Dispatch(
						call_dispatch::Event::<TestRuntime>::MessageSignatureMismatch(
							SOURCE_CHAIN_ID,
							id
						)
					),
					topics: vec![],
				}],
			);
		});
	}

	#[test]
	fn should_emit_event_for_rejected_messages() {
		new_test_ext().execute_with(|| {
			let id = [0; 4];
			let relayer_account = 1;

			System::set_block_number(1);
			Dispatch::dispatch(
				SOURCE_CHAIN_ID,
				TARGET_CHAIN_ID,
				&relayer_account,
				id,
				Err(()),
				|_, _| unreachable!(),
			);

			assert_eq!(
				System::events(),
				vec![EventRecord {
					phase: Phase::Initialization,
					event: RuntimeEvent::Dispatch(
						call_dispatch::Event::<TestRuntime>::MessageRejected(SOURCE_CHAIN_ID, id)
					),
					topics: vec![],
				}],
			);
		});
	}

	#[test]
	fn should_fail_on_call_decode() {
		new_test_ext().execute_with(|| {
			let id = [0; 4];
			let relayer_account = 1;

			let mut message =
				prepare_root_message(RuntimeCall::System(frame_system::Call::remark {
					remark: vec![1, 2, 3],
				}));
			let weight = message.weight;
			message.call.0 = vec![];

			System::set_block_number(1);
			let result = Dispatch::dispatch(
				SOURCE_CHAIN_ID,
				TARGET_CHAIN_ID,
				&relayer_account,
				id,
				Ok(message),
				|_, _| unreachable!(),
			);
			assert_eq!(result.unspent_weight, weight);
			assert!(!result.dispatch_result);

			assert_eq!(
				System::events(),
				vec![EventRecord {
					phase: Phase::Initialization,
					event: RuntimeEvent::Dispatch(
						call_dispatch::Event::<TestRuntime>::MessageCallDecodeFailed(
							SOURCE_CHAIN_ID,
							id
						)
					),
					topics: vec![],
				}],
			);
		});
	}

	#[test]
	fn should_emit_event_for_rejected_calls() {
		new_test_ext().execute_with(|| {
			let id = [0; 4];
			let relayer_account = 1;

			let call = RuntimeCall::RootTesting(pallet_root_testing::Call::fill_block {
				ratio: Perbill::from_percent(75),
			});
			let weight = call.get_dispatch_info().weight;
			let mut message = prepare_root_message(call);
			message.weight = weight;

			System::set_block_number(1);
			let result = Dispatch::dispatch(
				SOURCE_CHAIN_ID,
				TARGET_CHAIN_ID,
				&relayer_account,
				id,
				Ok(message),
				|_, _| unreachable!(),
			);
			assert_eq!(result.unspent_weight, weight);
			assert!(!result.dispatch_result);

			assert_eq!(
				System::events(),
				vec![EventRecord {
					phase: Phase::Initialization,
					event: RuntimeEvent::Dispatch(
						call_dispatch::Event::<TestRuntime>::MessageCallValidateFailed(
							SOURCE_CHAIN_ID,
							id,
							TransactionValidityError::Invalid(InvalidTransaction::Call),
						)
					),
					topics: vec![],
				}],
			);
		});
	}

	#[test]
	fn should_emit_event_for_unpaid_calls() {
		new_test_ext().execute_with(|| {
			let id = [0; 4];
			let relayer_account = 1;

			let mut message =
				prepare_root_message(RuntimeCall::System(frame_system::Call::remark {
					remark: vec![1, 2, 3],
				}));

			let weight = message.weight;
			message.dispatch_fee_payment = DispatchFeePayment::AtTargetChain;

			System::set_block_number(1);
			let result = Dispatch::dispatch(
				SOURCE_CHAIN_ID,
				TARGET_CHAIN_ID,
				&relayer_account,
				id,
				Ok(message),
				|_, _| Err(()),
			);
			assert_eq!(result.unspent_weight, weight);
			assert!(!result.dispatch_result);

			assert_eq!(
				System::events(),
				vec![EventRecord {
					phase: Phase::Initialization,
					event: RuntimeEvent::Dispatch(
						call_dispatch::Event::<TestRuntime>::MessageDispatchPaymentFailed(
							SOURCE_CHAIN_ID,
							id,
							AccountIdConverter::convert(derive_account_id::<AccountId>(
								SOURCE_CHAIN_ID,
								SourceAccount::Root
							)),
							TEST_WEIGHT,
						)
					),
					topics: vec![],
				}],
			);
		});
	}

	#[test]
	fn should_dispatch_calls_paid_at_target_chain() {
		new_test_ext().execute_with(|| {
			let id = [0; 4];
			let relayer_account = 1;

			let mut message =
				prepare_root_message(RuntimeCall::System(frame_system::Call::remark {
					remark: vec![1, 2, 3],
				}));
			message.dispatch_fee_payment = DispatchFeePayment::AtTargetChain;

			System::set_block_number(1);
			let result = Dispatch::dispatch(
				SOURCE_CHAIN_ID,
				TARGET_CHAIN_ID,
				&relayer_account,
				id,
				Ok(message),
				|_, _| Ok(()),
			);
			assert!(result.dispatch_fee_paid_during_dispatch);
			assert!(result.dispatch_result);

			assert_eq!(
				System::events(),
				vec![EventRecord {
					phase: Phase::Initialization,
					event: RuntimeEvent::Dispatch(
						call_dispatch::Event::<TestRuntime>::MessageDispatched(
							SOURCE_CHAIN_ID,
							id,
							Ok(())
						)
					),
					topics: vec![],
				}],
			);
		});
	}

	#[test]
	fn should_return_dispatch_failed_flag_if_dispatch_happened_but_failed() {
		new_test_ext().execute_with(|| {
			let id = [0; 4];
			let relayer_account = 1;

			let call = RuntimeCall::System(frame_system::Call::set_heap_pages { pages: 1 });
			let message = prepare_target_message(call);

			System::set_block_number(1);
			let result = Dispatch::dispatch(
				SOURCE_CHAIN_ID,
				TARGET_CHAIN_ID,
				&relayer_account,
				id,
				Ok(message),
				|_, _| unreachable!(),
			);
			assert!(!result.dispatch_fee_paid_during_dispatch);
			assert!(!result.dispatch_result);

			assert_eq!(
				System::events(),
				vec![EventRecord {
					phase: Phase::Initialization,
					event: RuntimeEvent::Dispatch(
						call_dispatch::Event::<TestRuntime>::MessageDispatched(
							SOURCE_CHAIN_ID,
							id,
							Err(sp_runtime::DispatchError::BadOrigin)
						)
					),
					topics: vec![],
				}],
			);
		})
	}

	#[test]
	fn should_dispatch_bridge_message_from_root_origin() {
		new_test_ext().execute_with(|| {
			let id = [0; 4];
			let relayer_account = 1;
			let message = prepare_root_message(RuntimeCall::System(frame_system::Call::remark {
				remark: vec![1, 2, 3],
			}));

			System::set_block_number(1);
			let result = Dispatch::dispatch(
				SOURCE_CHAIN_ID,
				TARGET_CHAIN_ID,
				&relayer_account,
				id,
				Ok(message),
				|_, _| unreachable!(),
			);
			assert!(!result.dispatch_fee_paid_during_dispatch);
			assert!(result.dispatch_result);

			assert_eq!(
				System::events(),
				vec![EventRecord {
					phase: Phase::Initialization,
					event: RuntimeEvent::Dispatch(
						call_dispatch::Event::<TestRuntime>::MessageDispatched(
							SOURCE_CHAIN_ID,
							id,
							Ok(())
						)
					),
					topics: vec![],
				}],
			);
		});
	}

	#[test]
	fn should_dispatch_bridge_message_from_target_origin() {
		new_test_ext().execute_with(|| {
			let id = [0; 4];
			let relayer_account = 1;

			let call = RuntimeCall::System(frame_system::Call::remark { remark: vec![] });
			let message = prepare_target_message(call);

			System::set_block_number(1);
			let result = Dispatch::dispatch(
				SOURCE_CHAIN_ID,
				TARGET_CHAIN_ID,
				&relayer_account,
				id,
				Ok(message),
				|_, _| unreachable!(),
			);
			assert!(!result.dispatch_fee_paid_during_dispatch);
			assert!(result.dispatch_result);

			assert_eq!(
				System::events(),
				vec![EventRecord {
					phase: Phase::Initialization,
					event: RuntimeEvent::Dispatch(
						call_dispatch::Event::<TestRuntime>::MessageDispatched(
							SOURCE_CHAIN_ID,
							id,
							Ok(())
						)
					),
					topics: vec![],
				}],
			);
		})
	}

	#[test]
	fn should_dispatch_bridge_message_from_source_origin() {
		new_test_ext().execute_with(|| {
			let id = [0; 4];
			let relayer_account = 1;

			let call = RuntimeCall::System(frame_system::Call::remark { remark: vec![] });
			let message = prepare_source_message(call);

			System::set_block_number(1);
			let result = Dispatch::dispatch(
				SOURCE_CHAIN_ID,
				TARGET_CHAIN_ID,
				&relayer_account,
				id,
				Ok(message),
				|_, _| unreachable!(),
			);
			assert!(!result.dispatch_fee_paid_during_dispatch);
			assert!(result.dispatch_result);

			assert_eq!(
				System::events(),
				vec![EventRecord {
					phase: Phase::Initialization,
					event: RuntimeEvent::Dispatch(
						call_dispatch::Event::<TestRuntime>::MessageDispatched(
							SOURCE_CHAIN_ID,
							id,
							Ok(())
						)
					),
					topics: vec![],
				}],
			);
		})
	}

	#[test]
	fn origin_is_checked_when_verifying_sending_message_using_source_root_account() {
		let call = RuntimeCall::System(frame_system::Call::remark { remark: vec![] });
		let message = prepare_root_message(call);

		// When message is sent by Root, CallOrigin::SourceRoot is allowed
		assert!(matches!(verify_message_origin(&RawOrigin::Root, &message), Ok(None)));

		// when message is sent by some real account, CallOrigin::SourceRoot is not allowed
		assert!(matches!(verify_message_origin(&RawOrigin::Signed(1), &message), Err(BadOrigin)));
	}

	#[test]
	fn origin_is_checked_when_verifying_sending_message_using_target_account() {
		let call = RuntimeCall::System(frame_system::Call::remark { remark: vec![] });
		let message = prepare_target_message(call);

		// When message is sent by Root, CallOrigin::TargetAccount is not allowed
		assert!(matches!(verify_message_origin(&RawOrigin::Root, &message), Err(BadOrigin)));

		// When message is sent by some other account, it is rejected
		assert!(matches!(verify_message_origin(&RawOrigin::Signed(2), &message), Err(BadOrigin)));

		// When message is sent by a real account, it is allowed to have origin
		// CallOrigin::TargetAccount
		assert!(matches!(verify_message_origin(&RawOrigin::Signed(1), &message), Ok(Some(1))));
	}

	#[test]
	fn origin_is_checked_when_verifying_sending_message_using_source_account() {
		let call = RuntimeCall::System(frame_system::Call::remark { remark: vec![] });
		let message = prepare_source_message(call);

		// Sending a message from the expected origin account works
		assert!(matches!(verify_message_origin(&RawOrigin::Signed(1), &message), Ok(Some(1))));

		// If we send a message from a different account, it is rejected
		assert!(matches!(verify_message_origin(&RawOrigin::Signed(2), &message), Err(BadOrigin)));

		// The Root account is allowed to assume any expected origin account
		assert!(matches!(verify_message_origin(&RawOrigin::Root, &message), Ok(Some(1))));
	}
}
