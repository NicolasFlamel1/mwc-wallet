// Copyright 2019 The vault713 Developers
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use super::bitcoin::{BtcBuyerContext, BtcData, BtcSellerContext};
use super::ser::*;
use super::ErrorKind;
use crate::swap::message::Message;
use bitcoin::Address;
use grin_core::global::ChainTypes;
use grin_core::{global, ser};
use grin_keychain::Identifier;
use grin_util::secp::key::SecretKey;
use std::convert::TryFrom;
use std::convert::TryInto;
use std::fmt;
use std::str::FromStr;

/// MWC Network where SWAP happens.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Network {
	/// Floonet (testnet)
	Floonet,
	/// Mainnet (production)
	Mainnet,
}

impl Network {
	/// Construct from current chaintype
	pub fn current_network() -> Result<Self, ErrorKind> {
		Ok(Self::from_chain_type(global::get_chain_type())?)
	}

	/// Constructor from mwc-node ChainTypes
	pub fn from_chain_type(chain_type: ChainTypes) -> Result<Self, ErrorKind> {
		match chain_type {
			ChainTypes::Floonet => Ok(Network::Floonet),
			ChainTypes::Mainnet => Ok(Network::Mainnet),
			_ => Err(ErrorKind::UnexpectedNetwork(format!("{:?}", chain_type))),
		}
	}
	/// To mwc-node ChainType
	pub fn to_chain_type(&self) -> ChainTypes {
		match self {
			Network::Floonet => ChainTypes::Floonet,
			Network::Mainnet => ChainTypes::Mainnet,
		}
	}
}

impl PartialEq<ChainTypes> for Network {
	fn eq(&self, other: &ChainTypes) -> bool {
		self.to_chain_type() == *other
	}
}

impl PartialEq<Network> for ChainTypes {
	fn eq(&self, other: &Network) -> bool {
		*self == other.to_chain_type()
	}
}

/// Roles of MWC swap participants
#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum Role {
	/// Seller - sell MWC for BTC. Params: (<Secondary redeem address>, <change amount>)
	Seller(String, u64),
	/// Buyer  - buy MWC for BTC. Params: (<Refund Address for Secondary>)
	Buyer(Option<String>),
}

/// Secondary currency that swap supports
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Currency {
	/// Bitcoin Segwit
	Btc,
	/// Bitcoin Cash
	Bch,
}

impl Currency {
	/// Satoshi to 1 conversion
	pub fn exponent(&self) -> usize {
		match self {
			Currency::Btc | Currency::Bch => 8,
		}
	}

	/// Block period for this coin (seconds)
	pub fn block_time_period_sec(&self) -> i64 {
		match self {
			Currency::Btc | Currency::Bch => 10 * 60,
		}
	}

	/// Print amount in nano coins normally
	pub fn amount_to_hr_string(&self, amount: u64, truncate: bool) -> String {
		let exp = self.exponent();
		let a = format!("{}", amount);
		let len = a.len();
		let pos = len.saturating_sub(exp);
		let (characteristic, mantissa_prefix) = if pos > 0 {
			(&a[..(len - exp)], String::new())
		} else {
			("0", "0".repeat(exp - len))
		};
		let mut mantissa = &a[pos..];
		if truncate {
			let nzeroes = mantissa.chars().rev().take_while(|c| c == &'0').count();
			mantissa = &a[pos..(a.len().saturating_sub(nzeroes))];
			if mantissa.len() == 0 {
				mantissa = "0";
			}
		}
		format!("{}.{}{}", characteristic, mantissa_prefix, mantissa)
	}

	/// Convert string amount to Satoshi amount
	pub fn amount_from_hr_string(&self, hr: &str) -> Result<u64, ErrorKind> {
		if hr.find(",").is_some() {
			return Err(ErrorKind::InvalidAmountString(hr.to_string()));
		}

		let exp = self.exponent();

		let (characteristic, mantissa) = match hr.find(".") {
			Some(pos) => {
				let (c, m) = hr.split_at(pos);
				(parse_characteristic(c)?, parse_mantissa(&m[1..], exp)?)
			}
			None => (parse_characteristic(hr)?, 0),
		};

		let amount = characteristic * 10u64.pow(exp as u32) + mantissa;
		if amount == 0 {
			return Err(ErrorKind::InvalidAmountString("zero amoount".to_string()));
		}

		Ok(amount)
	}

	fn bch_network() -> bch::network::Network {
		if global::is_mainnet() {
			bch::network::Network::Mainnet
		} else {
			bch::network::Network::Testnet
		}
	}

	/// Validate the secondary address
	pub fn validate_address(&self, address: &String) -> Result<(), ErrorKind> {
		match self {
			Currency::Btc => {
				let addr = Address::from_str(address).map_err(|e| {
					ErrorKind::Generic(format!("Unable to parse BTC address {}, {}", address, e))
				})?;
				match addr.network {
					bitcoin::network::constants::Network::Bitcoin => {
						if !global::is_mainnet() {
							return Err(ErrorKind::Generic(
								"Address is from main BTC network, expected test network"
									.to_string(),
							));
						}
					}
					bitcoin::network::constants::Network::Testnet => {
						if global::is_mainnet() {
							return Err(ErrorKind::Generic(
								"Address is from test BTC network, expected main network"
									.to_string(),
							));
						}
					}
					_ => {
						return Err(ErrorKind::Generic(
							"Address is from invalid BTC network".to_string(),
						))
					}
				}

				match addr.payload {
					bitcoin::util::address::Payload::PubkeyHash(_) => (),
					_ => {
						return Err(ErrorKind::Generic(
							"Expected BTC Pay-to-public-key-hash address".to_string(),
						))
					}
				}
			}
			Currency::Bch => {
				let nw = Self::bch_network();
				let (v, addr_type) = match bch::address::cashaddr_decode(&address, nw) {
					Err(e) => {
						// Try legacy address
						// Intentionally return error from first call. Legacy address error is not interesting much
						let (hash, addr_type) = bch::address::legacyaddr_decode(&address, nw)
							.map_err(|_| {
								ErrorKind::Generic(format!(
									"Unable to parse BCH address {}, {}",
									address, e
								))
							})?;
						(hash.0.to_vec(), addr_type)
					}
					Ok((v, addr_type)) => (v, addr_type),
				};
				if addr_type != bch::address::AddressType::P2PKH {
					return Err(ErrorKind::Generic(
						"Expected BTC Pay-to-public-key-hash address".to_string(),
					));
				}
				if v.len() != 160 / 8 {
					return Err(ErrorKind::Generic(
						"Swap supporting only Legacy of 160 bit BCH addresses".to_string(),
					));
				}
			}
		}
		Ok(())
	}

	/// Generate a script for this address. Address MUST be Hash160
	pub fn address_2_script_pubkey(&self, address: &String) -> Result<bitcoin::Script, ErrorKind> {
		let addr_str = match self {
			Currency::Btc => address.clone(),
			Currency::Bch => {
				// With BCH problem that it doesn't have functionality to build scripts for pay to pubkey
				// That is why we will use BTC library to do that.
				// In order to do that, we need to have legacy address.
				match bch::address::cashaddr_decode(&address, Self::bch_network()) {
					Err(_) => {
						// Legacy address - that is what we need
						address.clone()
					}
					Ok((v, addr_type)) => {
						if v.len() != 160 / 8 {
							return Err(ErrorKind::Generic(
								"Swap supporting only Legacy of 160 bit BCH addresses".to_string(),
							));
						}

						let ba: Box<[u8; 20]> = v.into_boxed_slice().try_into().map_err(|_| {
							ErrorKind::Generic(
								"Internal error. Failed to convert address to hash".to_string(),
							)
						})?;
						let hash_dt: [u8; 20] = *ba;

						let hash160 = bch::util::Hash160(hash_dt);
						// Converting into legacy address that is equal to BTC.
						bch::address::legacyaddr_encode(&hash160, addr_type, Self::bch_network())
					}
				}
			}
		};

		let addr = Address::from_str(&addr_str).map_err(|e| {
			ErrorKind::Generic(format!("Unable to parse BTC address {}, {}", address, e))
		})?;
		Ok(addr.script_pubkey())
	}

	/// Return default fee for this coin
	pub fn get_default_fee(&self, network: &Network) -> f32 {
		match self {
			Currency::Btc => {
				// Default values
				match network {
					Network::Floonet => 1.4 as f32,
					Network::Mainnet => 26.0 as f32,
				}
			}
			Currency::Bch => {
				// Default values
				match network {
					Network::Floonet => 1.4 as f32,
					Network::Mainnet => 24.0 as f32, // It is current average fee for BCH network, August 2020
				}
			}
		}
	}

	/// Fee units for this coin
	pub fn get_fee_units(&self) -> String {
		match self {
			Currency::Btc | Currency::Bch => "satoshi per byte".to_string(),
		}
	}

	/// Transaction at the first block. That transaction confirmation number must match the height of the chain
	pub fn get_block1_tx_hash(&self, testnet: bool) -> String {
		// Bch is clone of BTC, so even the same transaction does exist. For other alts that will not be true
		if testnet {
			match self {
				Currency::Btc | Currency::Bch => {
					"f0315ffc38709d70ad5647e22048358dd3745f3ce3874223c80a7c92fab0c8ba".to_string()
				}
			}
		} else {
			match self {
				Currency::Btc | Currency::Bch => {
					"0e3e2357e806b6cdb1f70b54c3a3a17b6714ee1f0e68bebb44a74b1efd512098".to_string()
				}
			}
		}
	}
}

impl fmt::Display for Currency {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		let disp = match &self {
			Currency::Btc => "BTC",
			Currency::Bch => "BCH",
		};
		write!(f, "{}", disp)
	}
}

impl TryFrom<&str> for Currency {
	type Error = ErrorKind;

	fn try_from(value: &str) -> Result<Self, Self::Error> {
		match value.to_lowercase().as_str() {
			"btc" => Ok(Currency::Btc),
			"bch" => Ok(Currency::Bch),
			_ => Err(ErrorKind::InvalidCurrency(value.to_string())),
		}
	}
}

fn parse_characteristic(characteristic: &str) -> Result<u64, ErrorKind> {
	if characteristic.len() == 0 {
		return Ok(0);
	}

	characteristic
		.parse()
		.map_err(|_| ErrorKind::InvalidAmountString(characteristic.to_string()))
}

fn parse_mantissa(mantissa: &str, exp: usize) -> Result<u64, ErrorKind> {
	let mut m = format!("{:0<w$}", mantissa, w = exp);
	m.truncate(exp);

	let m = m.trim_start_matches("0");
	if m.len() == 0 {
		return Ok(0);
	}
	m.parse()
		.map_err(|_| ErrorKind::InvalidAmountString(m.to_string()))
}

/// Secondary currency related data
#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum SecondaryData {
	/// None
	Empty,
	/// Bitcoin data
	Btc(BtcData),
}

impl SecondaryData {
	/// To BTC data
	pub fn unwrap_btc(&self) -> Result<&BtcData, ErrorKind> {
		match self {
			SecondaryData::Btc(d) => Ok(d),
			_ => Err(ErrorKind::UnexpectedCoinType),
		}
	}
	/// To BTC data
	pub fn unwrap_btc_mut(&mut self) -> Result<&mut BtcData, ErrorKind> {
		match self {
			SecondaryData::Btc(d) => Ok(d),
			_ => Err(ErrorKind::UnexpectedCoinType),
		}
	}
}

/// Buyer/Seller single deal context
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Context {
	/// Multisig key. Both Buyer and Seller using it
	pub multisig_key: Identifier,
	/// Multisig nonce. Both Buyer and Seller using it
	#[serde(serialize_with = "seckey_to_hex", deserialize_with = "seckey_from_hex")]
	pub multisig_nonce: SecretKey,
	/// Nonce that is Required to build a lock slate. Both sides need it.
	#[serde(serialize_with = "seckey_to_hex", deserialize_with = "seckey_from_hex")]
	pub lock_nonce: SecretKey,
	/// Nonce that is Required to build a refund slate. Both sides need it.
	#[serde(serialize_with = "seckey_to_hex", deserialize_with = "seckey_from_hex")]
	pub refund_nonce: SecretKey,
	/// Nonce that is Required to build a redeem slate. Both sides need it.
	#[serde(serialize_with = "seckey_to_hex", deserialize_with = "seckey_from_hex")]
	pub redeem_nonce: SecretKey,
	/// Specific Buyer or Seller context
	pub role_context: RoleContext,
}

impl Context {
	/// To Seller Context
	pub fn unwrap_seller(&self) -> Result<&SellerContext, ErrorKind> {
		match &self.role_context {
			RoleContext::Seller(c) => Ok(c),
			RoleContext::Buyer(_) => Err(ErrorKind::UnexpectedRole(
				"Context Fn unwrap_seller()".to_string(),
			)),
		}
	}

	/// To Buyer Context
	pub fn unwrap_buyer(&self) -> Result<&BuyerContext, ErrorKind> {
		match &self.role_context {
			RoleContext::Seller(_) => Err(ErrorKind::UnexpectedRole(
				"Context Fn unwrap_seller()".to_string(),
			)),
			RoleContext::Buyer(c) => Ok(c),
		}
	}
}

impl ser::Writeable for Context {
	fn write<W: ser::Writer>(&self, writer: &mut W) -> Result<(), ser::Error> {
		writer.write_bytes(&serde_json::to_vec(self).map_err(|e| {
			ser::Error::CorruptedData(format!("OutputData to json conversion failed, {}", e))
		})?)
	}
}

impl ser::Readable for Context {
	fn read<R: ser::Reader>(reader: &mut R) -> Result<Context, ser::Error> {
		let data = reader.read_bytes_len_prefix()?;
		serde_json::from_slice(&data[..]).map_err(|e| {
			ser::Error::CorruptedData(format!("Json to OutputData conversion failed, {}", e))
		})
	}
}

/// Context specfic to the swap party role
#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum RoleContext {
	/// Seller role
	Seller(SellerContext),
	/// Buyer role
	Buyer(BuyerContext),
}

/// Context for the seller party
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SellerContext {
	/// Seller account. Both lock and redeem will go to the same account (not receive)
	pub parent_key_id: Identifier,
	/// MWC Inputs that we are agree to sell: <Id, mmr_index (if known), amount>
	pub inputs: Vec<(Identifier, Option<u64>, u64)>,
	/// MWC Change outputs from the lock slate (Derivative ID)
	pub change_output: Identifier,
	/// Lock slate change amount
	pub change_amount: u64,
	/// MWC refund output  (Derivative ID)
	pub refund_output: Identifier,
	/// Secondary currency (BTC) related context
	pub secondary_context: SecondarySellerContext,
}

impl SellerContext {
	/// Retreive BTC data
	pub fn unwrap_btc(&self) -> Result<&BtcSellerContext, ErrorKind> {
		match &self.secondary_context {
			SecondarySellerContext::Btc(c) => Ok(c),
			//_ => Err(ErrorKind::UnexpectedCoinType),
		}
	}
}

/// Context for the Bayer party
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct BuyerContext {
	/// Buyer recieve account
	pub parent_key_id: Identifier,
	/// Derivative ID for lock slate output. Buyer part of multisig
	pub output: Identifier,
	/// Secret that unlocks the funds on both chains (Derivative ID)
	pub redeem: Identifier,
	/// Secondary currency (BTC) related context
	pub secondary_context: SecondaryBuyerContext,
}

impl BuyerContext {
	/// To BTC context
	pub fn unwrap_btc(&self) -> Result<&BtcBuyerContext, ErrorKind> {
		match &self.secondary_context {
			SecondaryBuyerContext::Btc(c) => Ok(c),
			//_ => Err(ErrorKind::UnexpectedCoinType),
		}
	}
}

/// Seller Secondary currency spepicif context
#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum SecondarySellerContext {
	/// BTC context
	Btc(BtcSellerContext),
}

/// Buyer secondary currency context
#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum SecondaryBuyerContext {
	/// BTC context
	Btc(BtcBuyerContext),
}

/// Action or step of the swap process
#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum Action {
	/// No further action required
	None,
	/// Seller Send a message to the counterparty
	SellerSendOfferMessage(Message),
	/// Buyer send accpet offer back to Sender
	BuyerSendAcceptOfferMessage(Message),

	/// Wait for a message from the counterparty
	SellerWaitingForOfferMessage,
	/// Buyer send to seller Init redeem message
	BuyerSendInitRedeemMessage(Message),
	/// Seller waiting for Init Redeem messgae
	SellerWaitingForInitRedeemMessage,
	/// Seller sending InitRedeemMessage
	SellerSendRedeemMessage(Message),
	/// Buyer waiting for Redeem Message
	BuyerWaitingForRedeemMessage,
	/// Waiting for the backup is done by user
	WaitingForTradeBackup,
	/// Seller Publishing an MWC lock transaction to the network
	SellerPublishMwcLockTx,
	/// Seller Publishing BTC redeem transaction to the network
	SellerPublishTxSecondaryRedeem(Currency),
	/// Deposit secondary currency
	DepositSecondary {
		/// Type of currency (BTC)
		currency: Currency,
		/// Amount
		amount: u64,
		/// Address to deposit
		address: String,
	},
	/// Wait for sufficient confirmations. Lock transaction on MWC network
	WaitForMwcConfirmations {
		/// What exactly are we waiting for.
		name: String,
		/// Required number of confirmations
		required: u64,
		/// Actual number of confirmations
		actual: u64,
	},
	/// Wait for sufficient confirmations on the secondary currency
	WaitForSecondaryConfirmations {
		/// What exactly are we waiting for.
		name: String,
		/// Expected amount to be posted
		expected_to_be_posted: u64,
		/// Type of currency (BTC)
		currency: Currency,
		/// Required number of confirmations
		required: u64,
		/// Actual number of confirmations
		actual: u64,
	},
	/// Wait for sufficient confirmations. Lock transaction on MWC network
	WaitForLockConfirmations {
		/// Required number of confirmations
		mwc_required: u64,
		/// Actual number of confirmations
		mwc_actual: u64,
		/// Type of secondary currency (BTC)
		currency: Currency,
		/// Expected amount to be posted
		sec_expected_to_be_posted: u64,
		/// Required number of confirmations for secondary
		sec_required: u64,
		/// Actual number of confirmations for secondary. None if secondary not posted
		sec_actual: Option<u64>,
	},
	/// Wait for the MWC redeem tx to be mined
	SellerWaitForBuyerRedeemPublish {
		/// Current mwc tip height
		mwc_tip: u64,
		/// Locking height
		lock_height: u64,
	},
	/// Wait for the Grin redeem tx to be mined
	WaitForMwcRefundUnlock {
		/// Current mwc tip height
		mwc_tip: u64,
		/// Locking height
		lock_height: u64,
	},

	/// Buyer publishing MWC redeem transaction and reveal the secret.
	BuyerPublishMwcRedeemTx,

	/// Seller Publishing MWC Refund Tx to the network
	SellerPublishMwcRefundTx,

	/// Buyer publishing refund transaction
	BuyerPublishSecondaryRefundTx(Currency),

	/// Waiting for btc refund to pass through
	WaitingForBtcRefund {
		/// Type of currency (BTC)
		currency: Currency,
		/// Required time (tiemstamp)
		required: u64,
		/// Current (tiemstamp)
		current: u64,
	},
}

impl Action {
	/// Return if this Action is None
	pub fn is_none(&self) -> bool {
		match &self {
			Action::None => true,
			_ => false,
		}
	}

	/// Return true if this action require execution (swap --process) from the user.
	pub fn can_execute(&self) -> bool {
		match &self {
			Action::SellerSendOfferMessage(_)
			| Action::BuyerSendAcceptOfferMessage(_)
			| Action::BuyerSendInitRedeemMessage(_)
			| Action::SellerSendRedeemMessage(_)
			| Action::SellerPublishMwcLockTx
			| Action::SellerPublishTxSecondaryRedeem(_)
			| Action::BuyerPublishMwcRedeemTx
			| Action::SellerPublishMwcRefundTx
			| Action::BuyerPublishSecondaryRefundTx(_) => true,
			_ => false,
		}
	}

	/// Convert action to a name string
	pub fn get_id_str(&self) -> String {
		let res = match &self {
			Action::None => "None",
			Action::SellerSendOfferMessage(_) => "SellerSendOfferMessage",
			Action::BuyerSendAcceptOfferMessage(_) => "BuyerSendAcceptOfferMessage",
			Action::SellerWaitingForOfferMessage => "SellerWaitForOfferMessage",
			Action::BuyerSendInitRedeemMessage(_) => "BuyerSendInitRedeemMessage",
			Action::SellerWaitingForInitRedeemMessage => "SellerWaitingForInitRedeemMessage",
			Action::SellerSendRedeemMessage(_) => "SellerSendRedeemMessage",
			Action::BuyerWaitingForRedeemMessage => "BuyerWaitingForRedeemMessage",
			Action::WaitingForTradeBackup => "WaitingForTradeBackup",
			Action::SellerPublishMwcLockTx => "SellerPublishMwcLockTx",
			Action::SellerPublishTxSecondaryRedeem(_) => "SellerPublishTxSecondaryRedeem",
			Action::DepositSecondary {
				currency: _,
				amount: _,
				address: _,
			} => "DepositSecondary",
			Action::WaitForMwcConfirmations {
				name: _,
				required: _,
				actual: _,
			} => "WaitForMwcConfirmations",
			Action::WaitForSecondaryConfirmations {
				name: _,
				expected_to_be_posted: _,
				currency: _,
				required: _,
				actual: _,
			} => "WaitForSecondaryConfirmations",
			Action::WaitForLockConfirmations {
				mwc_required: _,
				mwc_actual: _,
				currency: _,
				sec_expected_to_be_posted: _,
				sec_required: _,
				sec_actual: _,
			} => "WaitForLockConfirmations",
			Action::SellerWaitForBuyerRedeemPublish {
				mwc_tip: _,
				lock_height: _,
			} => "SellerWaitForBuyerRedeemPublish",
			Action::WaitForMwcRefundUnlock {
				mwc_tip: _,
				lock_height: _,
			} => "WaitForMwcRefundUnlock",
			Action::BuyerPublishMwcRedeemTx => "BuyerPublishMwcRedeemTx",
			Action::SellerPublishMwcRefundTx => "SellerPublishMwcRefundTx",
			Action::BuyerPublishSecondaryRefundTx(_) => "BuyerPublishSecondaryRefundTx",
			Action::WaitingForBtcRefund {
				currency: _,
				required: _,
				current: _,
			} => "WaitingForBtcRefund",
		};
		res.to_string()
	}
}

impl fmt::Display for Action {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		let disp = match &self {
			Action::None => "None".to_string(),
			Action::SellerSendOfferMessage(_) => "Sending Offer message".to_string(),
			Action::BuyerSendAcceptOfferMessage(_) => "Sending Accept Offer message".to_string(),
			Action::SellerWaitingForOfferMessage => "Waiting for Accept Offer message, please make sure that your listener is running".to_string(),
			Action::BuyerSendInitRedeemMessage(_) => "Sending Init Redeem Message".to_string(),
			Action::SellerWaitingForInitRedeemMessage => {
				"Waiting for Init Redeem Message, please make sure that your listener is running".to_string()
			}
			Action::SellerSendRedeemMessage(_) => "Sending Finalize Redeem Message".to_string(),
			Action::BuyerWaitingForRedeemMessage => {
				"Waiting for Redeem response message from Seller".to_string()
			}
			Action::WaitingForTradeBackup => "Waiting when backup will be done".to_string(),
			Action::SellerPublishMwcLockTx => "Posting MWC lock transaction".to_string(),
			Action::SellerPublishTxSecondaryRedeem(currency) => {
				format!("Posting {} redeem transaction", currency)
			}
			Action::DepositSecondary {
				currency,
				amount,
				address,
			} => format!(
				"Please deposit exactly {} {} to {}",
				currency.amount_to_hr_string(*amount, true),
				currency,
				address
			),
			Action::WaitForMwcConfirmations {
				name,
				required,
				actual,
			} => format!(
				"{}, waiting for confirmations. MWC {}/{}",
				name, actual, required
			),
			Action::WaitForSecondaryConfirmations {
				name,
				expected_to_be_posted,
				currency,
				required,
				actual,
			} => {
				if *expected_to_be_posted == 0 {
					format!("{}, waiting for confirmations. {} {}/{}", name, currency, actual, required)
				}
				else {
					let posted_str = currency.amount_to_hr_string(*expected_to_be_posted, true);
					format!("{}, waiting for {} {} to be posted", name, posted_str, currency)
				}
			}
			Action::WaitForLockConfirmations {
				mwc_required,
				mwc_actual,
				currency,
				sec_expected_to_be_posted,
				sec_required,
				sec_actual,
			} => {
				let mwc_str = if mwc_actual >= mwc_required {
					"MWC are locked".to_string()
				}
				else {
					format!("MWC {}/{}", mwc_actual, mwc_required)
				};

				let sec_str = if *sec_expected_to_be_posted==0 {
					if sec_actual.unwrap() == 0 {
						format!("{} are in memory pool", currency)
					}
					else if sec_actual.unwrap() >= *sec_required {
						format!("{} are locked", currency)
					}
					else {
						format!("{} {}/{}", currency, sec_actual.unwrap(), sec_required)
					}
				}
				else {
					let sec_posted_str = currency.amount_to_hr_string(*sec_expected_to_be_posted, true);
					format!("Waiting for {} {} to be posted", sec_posted_str, currency)
				};

				format!("Locking, waiting for confirmations. {}; {}", mwc_str, sec_str)
			}
			Action::SellerWaitForBuyerRedeemPublish {
				mwc_tip,
				lock_height,
			} => format!("Waiting for Buyer to redeem MWC. If get no response, will post refund slate in {} minutes",(lock_height.saturating_sub(*mwc_tip))),
			Action::WaitForMwcRefundUnlock {
				mwc_tip,
				lock_height,
			} => {
				let blocks_left = lock_height.saturating_sub(*mwc_tip);
				let hours = blocks_left / 60;
				let minutes = blocks_left % 60;
				format!( "Waiting for locked MWC to be refunded ({} hour{} and {} minute{} remaining)", hours, if hours == 1 { "" } else { "s" }, minutes, if minutes == 1 { "" } else { "s" })
			},
			Action::BuyerPublishMwcRedeemTx => "Posting MWC redeem transaction".to_string(),
			Action::SellerPublishMwcRefundTx => "Posting MWC refund transaction".to_string(),
			Action::BuyerPublishSecondaryRefundTx(currency) => {
				format!("Posting {} refund transaction", currency)
			}
			Action::WaitingForBtcRefund {
				currency,
				required,
				current,
			} => {
				let time_left_sec = required - current + 30; // 30 seconds for rounding
				let hours = time_left_sec / 3600;
				let minutes = (time_left_sec % 3600) / 60;
				format!("Waiting for locked {} to be refunded. ({} hour{} and {} minute{} remaining)", currency, hours, if hours==1 {""} else {"s"} , minutes, if minutes==1 {""} else {"s"} )
			}
		};
		write!(f, "{}", disp)
	}
}

/// Status of the transactions that can be published.
///  None for confirmations - Unable to verify, probably Transaction data is not here.
pub struct SwapTransactionsConfirmations {
	/// MWC node tip
	pub mwc_tip: u64,
	/// Number of confirmations for the lock transaction
	pub mwc_lock_conf: Option<u64>,
	/// Number of confirmations for MWC redeem transaction
	pub mwc_redeem_conf: Option<u64>,
	/// Number of confirmations for refund transaction
	pub mwc_refund_conf: Option<u64>,
	/// BTC node tip
	pub secondary_tip: u64,
	/// BTC lock (multisug account) number of confirmations
	pub secondary_lock_conf: Option<u64>,
	/// How much is locked. This process is manual, so Buyer might make a mistake
	pub secondary_lock_amount: u64,
	/// BTC redeem number of confirmations
	pub secondary_redeem_conf: Option<u64>,
	/// BTC  refund transaciton number of confirmations
	pub secondary_refund_conf: Option<u64>,
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_amounts_to_hr() {
		let c = Currency::Btc;
		assert_eq!(&c.amount_to_hr_string(1, false), "0.00000001");
		assert_eq!(&c.amount_to_hr_string(100, false), "0.00000100");
		assert_eq!(&c.amount_to_hr_string(713, false), "0.00000713");
		assert_eq!(&c.amount_to_hr_string(100_000, false), "0.00100000");
		assert_eq!(&c.amount_to_hr_string(10_000_000, false), "0.10000000");
		assert_eq!(&c.amount_to_hr_string(12_345_678, false), "0.12345678");
		assert_eq!(&c.amount_to_hr_string(100_000_000, false), "1.00000000");
		assert_eq!(&c.amount_to_hr_string(100_200_300, false), "1.00200300");
		assert_eq!(&c.amount_to_hr_string(102_030_405, false), "1.02030405");
		assert_eq!(&c.amount_to_hr_string(110_000_000, false), "1.10000000");
		assert_eq!(&c.amount_to_hr_string(123_456_789, false), "1.23456789");
		assert_eq!(&c.amount_to_hr_string(1_000_000_000, false), "10.00000000");
		assert_eq!(&c.amount_to_hr_string(1_020_304_050, false), "10.20304050");
		assert_eq!(
			&c.amount_to_hr_string(10_000_000_000, false),
			"100.00000000"
		);
		assert_eq!(
			&c.amount_to_hr_string(10_000_000_001, false),
			"100.00000001"
		);
		assert_eq!(
			&c.amount_to_hr_string(10_000_000_010, false),
			"100.00000010"
		);
		assert_eq!(
			&c.amount_to_hr_string(10_000_000_100, false),
			"100.00000100"
		);

		assert_eq!(&c.amount_to_hr_string(1, true), "0.00000001");
		assert_eq!(&c.amount_to_hr_string(100, true), "0.000001");
		assert_eq!(&c.amount_to_hr_string(713, true), "0.00000713");
		assert_eq!(&c.amount_to_hr_string(100_000, true), "0.001");
		assert_eq!(&c.amount_to_hr_string(10_000_000, true), "0.1");
		assert_eq!(&c.amount_to_hr_string(12_345_678, true), "0.12345678");
		assert_eq!(&c.amount_to_hr_string(100_000_000, true), "1.0");
		assert_eq!(&c.amount_to_hr_string(100_200_300, true), "1.002003");
		assert_eq!(&c.amount_to_hr_string(102_030_405, true), "1.02030405");
		assert_eq!(&c.amount_to_hr_string(110_000_000, true), "1.1");
		assert_eq!(&c.amount_to_hr_string(123_456_789, true), "1.23456789");
		assert_eq!(&c.amount_to_hr_string(1_000_000_000, true), "10.0");
		assert_eq!(&c.amount_to_hr_string(1_020_304_050, true), "10.2030405");
		assert_eq!(&c.amount_to_hr_string(10_000_000_000, true), "100.0");
		assert_eq!(&c.amount_to_hr_string(10_000_000_001, true), "100.00000001");
		assert_eq!(&c.amount_to_hr_string(10_000_000_010, true), "100.0000001");
		assert_eq!(&c.amount_to_hr_string(10_000_000_100, true), "100.000001");
	}

	#[test]
	fn test_amounts_from_hr() {
		let c = Currency::Btc;
		assert!(c.amount_from_hr_string("").is_err());
		assert!(c.amount_from_hr_string(".").is_err());
		assert!(c.amount_from_hr_string("0").is_err());
		assert!(c.amount_from_hr_string("0.").is_err());
		assert!(c.amount_from_hr_string("0.0").is_err());
		assert!(c.amount_from_hr_string("0.000000001").is_err());
		assert_eq!(c.amount_from_hr_string("0.00000001").unwrap(), 1);
		assert_eq!(c.amount_from_hr_string(".00000001").unwrap(), 1);
		assert_eq!(c.amount_from_hr_string("0.00000713").unwrap(), 713);
		assert_eq!(c.amount_from_hr_string(".00000713").unwrap(), 713);
		assert_eq!(c.amount_from_hr_string("0.0001").unwrap(), 10_000);
		assert_eq!(c.amount_from_hr_string("0.1").unwrap(), 10_000_000);
		assert_eq!(c.amount_from_hr_string("0.10").unwrap(), 10_000_000);
		assert_eq!(c.amount_from_hr_string(".1").unwrap(), 10_000_000);
		assert_eq!(c.amount_from_hr_string(".10").unwrap(), 10_000_000);
		assert_eq!(c.amount_from_hr_string("0.123456789").unwrap(), 12_345_678);
		assert_eq!(c.amount_from_hr_string("1").unwrap(), 100_000_000);
		assert_eq!(c.amount_from_hr_string("1.").unwrap(), 100_000_000);
		assert_eq!(c.amount_from_hr_string("1.0").unwrap(), 100_000_000);
		assert_eq!(
			c.amount_from_hr_string("123456.789").unwrap(),
			12_345_678_900_000
		);
	}

	#[test]
	fn test_bch_address_parsers() {
		global::set_local_chain_type(ChainTypes::Floonet);

		let bch_q_address = "bchtest:qr972p5km7a9rdwtsnuqjfnm8epm48mhkgcgt6dprl".to_string();
		let bch_legacy = "mz73pyxw6hpnyb8HHnPrTe5DikC2xYrfPX".to_string();

		let btc_script = Currency::Btc.address_2_script_pubkey(&bch_legacy).unwrap();
		let bch_legacy_script = Currency::Bch.address_2_script_pubkey(&bch_legacy).unwrap();
		let bch_q_script = Currency::Bch
			.address_2_script_pubkey(&bch_q_address)
			.unwrap();

		let script_sz = btc_script.len();

		assert_eq!(script_sz, bch_legacy_script.len());
		assert_eq!(script_sz, bch_q_script.len());

		for i in 0..script_sz {
			assert_eq!(btc_script.as_bytes()[i], bch_legacy_script.as_bytes()[i]);
			assert_eq!(btc_script.as_bytes()[i], bch_q_script.as_bytes()[i]);
		}
	}
}
