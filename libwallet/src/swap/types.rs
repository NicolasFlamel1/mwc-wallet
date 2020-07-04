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
use grin_core::global::ChainTypes;
use grin_core::ser;
use grin_keychain::Identifier;
use grin_util::secp::key::SecretKey;
use std::convert::TryFrom;
use std::fmt;

/// MWC Network where SWAP happens.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Network {
	/// Floonet (testnet)
	Floonet,
	/// Mainnet (production)
	Mainnet,
}

impl Network {
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
	/// Seller - sell MWC for BTC. Params: (<BTC redeem address>, <change amount>)
	Seller(String, u64),
	/// Buyer  - buy MWC for BTC
	Buyer,
}

/// Status of the MWC swap session
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Status {
	/// Swap instance Created by Seller
	Created,
	/// Offered to Buyer
	Offered,
	/// Offer accepted by Seller
	Accepted,
	/// MWC & BTC funds are locked
	Locked,
	/// Init Redeem transaction
	InitRedeem,
	/// Buyer redeem MWC transaction
	Redeem,
	/// Seller redeem BTC Transaction
	RedeemSecondary,
	/// Done
	Completed,
	/// Failure scenario for Seller: MWC refund transaction was posted, get a refund
	Refunded,
	/// Failure scenario for both parties: Session is cancelled, might wait for Refund.
	Cancelled,
}

impl fmt::Display for Status {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		let disp = match &self {
			Status::Created => "created",
			Status::Offered => "offered",
			Status::Accepted => "accepted",
			Status::Locked => "locked",
			Status::InitRedeem => "init redeem",
			Status::Redeem => "buyer redeem",
			Status::RedeemSecondary => "seller redeem",
			Status::Completed => "completed",
			Status::Refunded => "refunded",
			Status::Cancelled => "cancelled",
		};
		write!(f, "{}", disp)
	}
}

/// Secondary currency that swap support
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Currency {
	/// Bitcoin Segwit
	Btc,
}

impl Currency {
	/// Satoshi to 1 conversion
	pub fn exponent(&self) -> usize {
		match self {
			Currency::Btc => 8,
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
}

impl fmt::Display for Currency {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		let disp = match &self {
			Currency::Btc => "BTC",
		};
		write!(f, "{}", disp)
	}
}

impl TryFrom<&str> for Currency {
	type Error = ErrorKind;

	fn try_from(value: &str) -> Result<Self, Self::Error> {
		match value.to_lowercase().as_str() {
			"btc" => Ok(Currency::Btc),
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
	m.parse().map_err(|_| ErrorKind::InvalidAmountString(m.to_string()))
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
	/// Nonce that is requred to build a lock slate. Both sides need it.
	#[serde(serialize_with = "seckey_to_hex", deserialize_with = "seckey_from_hex")]
	pub lock_nonce: SecretKey,
	/// Nonce that is requred to build a refund slate. Both sides need it.
	#[serde(serialize_with = "seckey_to_hex", deserialize_with = "seckey_from_hex")]
	pub refund_nonce: SecretKey,
	/// Nonce that is requred to build a redeem slate. Both sides need it.
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
			RoleContext::Buyer(_) => Err(ErrorKind::UnexpectedRole("Context Fn unwrap_seller()".to_string())),
		}
	}

	/// To Buyer Context
	pub fn unwrap_buyer(&self) -> Result<&BuyerContext, ErrorKind> {
		match &self.role_context {
			RoleContext::Seller(_) => Err(ErrorKind::UnexpectedRole( "Context Fn unwrap_seller()".to_string()) ),
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
	fn read(reader: &mut dyn ser::Reader) -> Result<Context, ser::Error> {
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
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum Action {
	/// No further action required
	None,
	/// Send a message to the counterparty
	SendMessage(usize),
	/// Wait for a message from the counterparty
	ReceiveMessage,
	/// Publish a transaction to the network
	PublishTx,
	/// Publish a transaction to the network of the secondary currency
	PublishTxSecondary(Currency),
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
	Confirmations {
		/// Required number of confirmations
		required: u64,
		/// Actual number of confirmations
		actual: u64,
	},
	/// Wait for sufficient confirmations on the secondary currency
	ConfirmationsSecondary {
		/// Type of currency (BTC)
		currency: Currency,
		/// Required number of confirmations
		required: u64,
		/// Actual number of confirmations
		actual: u64,
	},
	/// Wait for the Grin redeem tx to be mined
	ConfirmationRedeem,
	/// Wait for the secondary redeem tx to be mined
	ConfirmationRedeemSecondary(Currency, String),
	/// Complete swap
	Complete,
	/// Cancel swap
	Cancel,
	/// Waiting for refund to pass through
	WaitingForRefund{
		/// Required height
		required: u64,
		/// Actual height
		height: u64,
	},
	/// Execute refund
	Refund,
}

impl fmt::Display for Action {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		let disp = match &self {
			Action::None =>
				"Nothing to do".to_string(),
			Action::SendMessage(i) =>
				format!("(msg{}) Send Message {}", i, i),
			Action::ReceiveMessage =>
				"(receive) Waiting for respond from other party".to_string(),
			Action::PublishTx =>
				"(postlock) Need to post MWC Lock Transaction".to_string(),
			Action::PublishTxSecondary(currency) =>
				format!("(publish_{}) Publish a transaction for {}", currency, currency),
			Action::DepositSecondary{ currency, amount, address} =>
				format!("Deposit {} {} at {}", currency.amount_to_hr_string(*amount, true), currency, address ),
			Action::Confirmations{required, actual} =>
				format!("Waiting for {} MWC lock confirmations, has {}", required, actual),
			Action::ConfirmationsSecondary{currency, required, actual} =>
				format!("Waiting for {} {} lock confirmations, has {}", required, currency, actual),
			Action::ConfirmationRedeem =>
				"Waiting for MWC Redeem transaction to be confirmed".to_string(),
			Action::ConfirmationRedeemSecondary(currency, btc_address)  =>
				format!("Waiting for {} Redeem transaction to be confirmed for {}", currency, btc_address),
			Action::Complete =>
				"Swap trade is complete".to_string(),
			Action::Cancel =>
				"(cancel) Swap trade cancelled".to_string(),
			// Waiting for refund to pass through
			Action::WaitingForRefund{ required, height} => {
				let blocks_left = required - height;
				format!("Waiting for block {} to be ready to post refund slate, {} blocks are left", required, blocks_left)
			},
			Action::Refund =>
				"(refund) Refund can be issued".to_string(),
		};
		write!(f, "{}", disp)
	}
}

impl Action {
	/// String to the
	pub fn from_cmd(cmd: &str) -> Option<Action> {
		match cmd {
			"msg1" => Some( Action::SendMessage(1)),
			"msg2" => Some( Action::SendMessage(2)),
			"receive" => Some( Action::ReceiveMessage),
			"postlock" => Some( Action::PublishTx),
			"refund" => Some( Action::Refund ),
			"publish_btc" => Some(Action::PublishTxSecondary(Currency::Btc)),
			"cancel" => Some( Action::Cancel ),
			_ => None,
		}
	}

	/// Action to the command string. Only action that have require some user input, has this maping
	pub fn to_cmd(&self) -> Option<String> {
		match &self {
			Action::None |
				Action::ConfirmationRedeem |
				Action::Complete | Action::Cancel =>
				None,
			Action::DepositSecondary{currency: _, amount: _, address: _} => None,
			Action::ConfirmationRedeemSecondary(_currency, _btc_address) => None,
			Action::Confirmations{required: _, actual: _} => None,
			Action::ConfirmationsSecondary{currency: _, required: _, actual: _ } => None,
			Action::SendMessage(i) =>
				Some(format!("msg{}", i)),
			Action::ReceiveMessage =>
				Some("receive".to_string()),
			Action::PublishTx =>
				Some("postlock".to_string()),
			Action::Refund =>
				Some("refund".to_string()),
			_ => None,
		}
	}
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
}
