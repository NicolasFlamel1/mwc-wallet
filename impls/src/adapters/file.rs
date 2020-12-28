// Copyright 2019 The Grin Developers
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

/// File Output 'plugin' implementation
use std::fs::File;
use std::io::{Read, Write};

use crate::adapters::SlateGetData;
use crate::error::{Error, ErrorKind};
use crate::libwallet::{Slate, SlateVersion, VersionedSlate};
use crate::{SlateGetter, SlatePutter};
use ed25519_dalek::{PublicKey as DalekPublicKey, SecretKey as DalekSecretKey};
use grin_wallet_libwallet::slatepack::SlatePurpose;
use std::path::PathBuf;

#[derive(Clone)]
pub struct PathToSlatePutter {
	path_buf: PathBuf,
	content: Option<SlatePurpose>,
	sender: Option<DalekPublicKey>,
	recipient: Option<DalekPublicKey>,
}

pub struct PathToSlateGetter {
	path_buf: PathBuf,
}

impl PathToSlatePutter {
	// Build sender that can save slatepacks
	pub fn build_encrypted(
		path_buf: PathBuf,
		content: SlatePurpose,
		sender: DalekPublicKey,
		recipient: Option<DalekPublicKey>,
	) -> Self {
		Self {
			path_buf,
			content: Some(content),
			sender: Some(sender),
			recipient: recipient,
		}
	}

	pub fn build_plain(path_buf: PathBuf) -> Self {
		Self {
			path_buf,
			content: None,
			sender: None,
			recipient: None,
		}
	}
}

impl PathToSlateGetter {
	pub fn build(path_buf: PathBuf) -> Self {
		Self { path_buf }
	}
}

impl SlatePutter for PathToSlatePutter {
	fn put_tx(
		&self,
		slate: &Slate,
		slatepack_secret: &DalekSecretKey,
		use_test_rng: bool,
	) -> Result<(), Error> {
		let file_name = self.path_buf.to_str().unwrap_or("INVALID PATH");
		let mut pub_tx = File::create(&self.path_buf).map_err(|e| {
			ErrorKind::IO(format!("Unable to create proof file {}, {}", file_name, e))
		})?;

		let out_slate = {
			if self.recipient.is_some() {
				if self.sender.is_none() || self.content.is_none() {
					return Err(ErrorKind::GenericError(
						"Sender or content are not defined".to_string(),
					)
					.into());
				}

				// Do the slatepack
				VersionedSlate::into_version(
					slate.clone(),
					SlateVersion::SP,
					self.content.clone().unwrap(),
					self.sender.clone().unwrap(),
					self.recipient.clone(),
					slatepack_secret,
					use_test_rng,
				)
				.map_err(|e| {
					ErrorKind::GenericError(format!("Unable to build a slatepack, {}", e))
				})?
			} else if slate.compact_slate {
				warn!("Transaction contains features that require mwc-wallet 4.0.0 or later");
				warn!("Please ensure the other party is running mwc-wallet v4.0.0 or later before sending");
				VersionedSlate::into_version_plain(slate.clone(), SlateVersion::V3).map_err(
					|e| ErrorKind::GenericError(format!("Failed convert Slate to Json, {}", e)),
				)?
			} else if slate.payment_proof.is_some() || slate.ttl_cutoff_height.is_some() {
				warn!("Transaction contains features that require mwc-wallet 3.0.0 or later");
				warn!("Please ensure the other party is running mwc-wallet v3.0.0 or later before sending");
				VersionedSlate::into_version_plain(slate.clone(), SlateVersion::V3).map_err(
					|e| ErrorKind::GenericError(format!("Failed convert Slate to Json, {}", e)),
				)?
			} else {
				let mut s = slate.clone();
				s.version_info.version = 2;
				VersionedSlate::into_version_plain(s, SlateVersion::V2).map_err(|e| {
					ErrorKind::GenericError(format!("Failed convert Slate to Json, {}", e))
				})?
			}
		};
		pub_tx
			.write_all(out_slate.as_string()?.as_bytes())
			.map_err(|e| {
				ErrorKind::IO(format!(
					"Unable to store data at proof file {}, {}",
					file_name, e
				))
			})?;

		pub_tx.sync_all().map_err(|e| {
			ErrorKind::IO(format!(
				"Unable to store data at proof file {}, {}",
				file_name, e
			))
		})?;

		Ok(())
	}
}

impl SlateGetter for PathToSlateGetter {
	fn get_tx(&self, slatepack_secret: &DalekSecretKey) -> Result<SlateGetData, Error> {
		let file_name = self.path_buf.to_str().unwrap_or("INVALID PATH");
		let mut pub_tx_f = File::open(&self.path_buf).map_err(|e| {
			ErrorKind::IO(format!("Unable to open proof file {}, {}", file_name, e))
		})?;
		let mut content = String::new();
		pub_tx_f.read_to_string(&mut content).map_err(|e| {
			ErrorKind::IO(format!(
				"Unable to read data from file {}, {}",
				file_name, e
			))
		})?;

		if Slate::deserialize_is_plain(&content) {
			let slate = Slate::deserialize_upgrade_plain(&content).map_err(|e| {
				ErrorKind::IO(format!(
					"Unable to build slate from json, file {}, {}",
					file_name, e
				))
			})?;
			Ok(SlateGetData::PlainSlate(slate))
		} else {
			let sp = Slate::deserialize_upgrade_slatepack(&content, slatepack_secret)?;
			Ok(SlateGetData::Slatepack(sp))
		}
	}
}
