use crate::config::{Config, ElementsNetwork};
use crate::error::Error;
use crate::model::{Addressee, UnblindedTXO, TXO};
use crate::store::{new_store, Store};
use crate::sync::sync;
use crate::util::EC;
use electrum_client::bitcoin::bip32::{ChildNumber, DerivationPath, Fingerprint};
use electrum_client::ElectrumApi;
use elements::bitcoin::hashes::{sha256, Hash};
use elements::pset::{Input, Output, PartiallySignedTransaction};
use elements::{
    self, Address, AddressParams, AssetId, BlockHash, BlockHeader, OutPoint, Script, Transaction,
    TxOut, Txid,
};
use elements_miniscript::confidential::Key;
use elements_miniscript::{
    ConfidentialDescriptor, DefiniteDescriptorKey, DescriptorPublicKey, ForEachKey,
};
use rand::thread_rng;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;

pub(crate) fn derive_address(
    descriptor: &ConfidentialDescriptor<DescriptorPublicKey>,
    index: u32,
    address_params: &'static AddressParams,
) -> Result<Address, Error> {
    let derived_non_conf = descriptor.descriptor.at_derivation_index(index)?;

    let derived_conf = ConfidentialDescriptor::<DefiniteDescriptorKey> {
        key: convert_blinding_key(&descriptor.key)?,
        descriptor: derived_non_conf,
    };

    Ok(derived_conf.address(&EC, address_params)?)
}

pub(crate) fn derive_public_keys(
    descriptor: &ConfidentialDescriptor<DescriptorPublicKey>,
    index: u32,
) -> Result<Vec<elements::bitcoin::PublicKey>, Error> {
    let derived_non_conf = descriptor.descriptor.at_derivation_index(index)?;
    let mut keys = vec![];
    derived_non_conf.for_each_key(|k| {
        keys.push(k.derive_public_key(&EC).unwrap());
        true
    });
    Ok(keys)
}

fn convert_blinding_key(
    key: &Key<DescriptorPublicKey>,
) -> Result<Key<DefiniteDescriptorKey>, Error> {
    match key {
        Key::Slip77(x) => Ok(Key::Slip77(*x)),
        Key::Bare(_) => Err(Error::BlindingBareUnsupported),
        Key::View(x) => Ok(Key::View(x.clone())),
    }
}

pub struct ElectrumWallet {
    config: Config,
    store: Store,
    descriptor: ConfidentialDescriptor<DescriptorPublicKey>,
}

impl ElectrumWallet {
    /// Create a new  wallet
    pub fn new(
        network: ElementsNetwork,
        electrum_url: &str,
        tls: bool,
        validate_domain: bool,
        data_root: &str,
        desc: &str,
    ) -> Result<Self, Error> {
        let config = Config::new(network, tls, validate_domain, electrum_url, data_root)?;
        Self::inner_new(config, desc)
    }

    fn inner_new(config: Config, desc: &str) -> Result<Self, Error> {
        let descriptor = ConfidentialDescriptor::<DescriptorPublicKey>::from_str(desc)?;

        let wallet_desc = format!("{}{:?}", desc, config);
        let wallet_id = format!("{}", sha256::Hash::hash(wallet_desc.as_bytes()));

        let mut path: PathBuf = config.data_root().into();
        if !path.exists() {
            std::fs::create_dir_all(&path)?;
        }
        path.push(wallet_id);
        let store = new_store(&path, descriptor.clone())?;

        Ok(ElectrumWallet {
            store,
            config,
            descriptor,
        })
    }

    fn descriptor_blinding_key(&self) -> Key<DefiniteDescriptorKey> {
        convert_blinding_key(&self.descriptor.key)
            .expect("No private blinding keys for bare variant")
    }

    /// Get the network policy asset
    pub fn policy_asset(&self) -> AssetId {
        self.config.policy_asset()
    }

    /// Sync the wallet transactions
    pub fn sync_txs(&mut self) -> Result<(), Error> {
        if let Ok(client) = self.config.electrum_url().build_client() {
            let blinding_key = self.descriptor_blinding_key();
            match sync(&client, &mut self.store, blinding_key) {
                Ok(true) => log::info!("there are new transcations"),
                Ok(false) => (),
                Err(e) => log::warn!("Error during sync, {:?}", e),
            }
        }
        Ok(())
    }

    /// Sync the blockchain tip
    pub fn sync_tip(&mut self) -> Result<(), Error> {
        if let Ok(client) = self.config.electrum_url().build_client() {
            let header = client.block_headers_subscribe_raw()?;
            let height = header.height as u32;
            let tip_height = self.store.cache.tip.0;
            if height != tip_height {
                let block_header: BlockHeader = elements::encode::deserialize(&header.header)?;
                let hash: BlockHash = block_header.block_hash();
                self.store.cache.tip = (height, hash);
            }
        }
        Ok(())
    }

    /// Get the blockchain tip
    pub fn tip(&self) -> Result<(u32, BlockHash), Error> {
        Ok(self.store.cache.tip)
    }

    fn derive_address(&self, index: u32) -> Result<Address, Error> {
        derive_address(&self.descriptor, index, self.config.address_params())
    }

    /// Get a new wallet address
    pub fn address(&mut self) -> Result<Address, Error> {
        self.store.cache.last_index += 1;

        self.derive_address(self.store.cache.last_index)
    }

    /// Get the wallet UTXOs
    pub fn utxos(&self) -> Result<Vec<UnblindedTXO>, Error> {
        let mut txos = vec![];
        let spent = self.store.spent()?;
        for (tx_id, height) in self.store.cache.heights.iter() {
            let tx = self
                .store
                .cache
                .all_txs
                .get(tx_id)
                .ok_or_else(|| Error::Generic(format!("txos no tx {}", tx_id)))?;
            let tx_txos: Vec<UnblindedTXO> = {
                tx.output
                    .clone()
                    .into_iter()
                    .enumerate()
                    .map(|(vout, output)| {
                        (
                            OutPoint {
                                txid: tx.txid(),
                                vout: vout as u32,
                            },
                            output,
                        )
                    })
                    .filter(|(outpoint, _)| !spent.contains(outpoint))
                    .filter_map(|(outpoint, output)| {
                        if let Some(unblinded) = self.store.cache.unblinded.get(&outpoint) {
                            let txo = TXO::new(outpoint, output.script_pubkey, *height);
                            return Some(UnblindedTXO {
                                txo,
                                unblinded: *unblinded,
                            });
                        }
                        None
                    })
                    .collect()
            };
            txos.extend(tx_txos);
        }
        txos.sort_by(|a, b| b.unblinded.value.cmp(&a.unblinded.value));

        Ok(txos)
    }

    fn balance_from_utxos(&self, utxos: &[UnblindedTXO]) -> Result<HashMap<AssetId, u64>, Error> {
        let mut r = HashMap::new();
        r.entry(self.policy_asset()).or_insert(0);
        for u in utxos.iter() {
            *r.entry(u.unblinded.asset).or_default() += u.unblinded.value;
        }
        Ok(r)
    }

    /// Get the wallet balance
    pub fn balance(&self) -> Result<HashMap<AssetId, u64>, Error> {
        let utxos = self.utxos()?;
        self.balance_from_utxos(&utxos)
    }

    /// Get the wallet transactions with their heights (if confirmed)
    pub fn transactions(&self) -> Result<Vec<(Transaction, Option<u32>)>, Error> {
        let mut txs = vec![];
        let mut my_txids: Vec<(&Txid, &Option<u32>)> = self.store.cache.heights.iter().collect();
        my_txids.sort_by(|a, b| {
            let height_cmp =
                b.1.unwrap_or(std::u32::MAX)
                    .cmp(&a.1.unwrap_or(std::u32::MAX));
            match height_cmp {
                Ordering::Equal => b.0.cmp(a.0),
                h => h,
            }
        });

        for (tx_id, height) in my_txids.iter() {
            let tx = self
                .store
                .cache
                .all_txs
                .get(*tx_id)
                .ok_or_else(|| Error::Generic(format!("list_tx no tx {}", tx_id)))?;

            txs.push((tx.clone(), **height));
        }

        Ok(txs)
    }

    #[allow(dead_code)]
    fn asset_utxos(&self, asset: &AssetId) -> Result<Vec<UnblindedTXO>, Error> {
        Ok(self
            .utxos()?
            .into_iter()
            .filter(|utxo| &utxo.unblinded.asset == asset)
            .collect())
    }

    fn get_txout(&self, outpoint: &OutPoint) -> Result<TxOut, Error> {
        let tx = self
            .store
            .cache
            .all_txs
            .get(&outpoint.txid)
            .ok_or_else(|| Error::MissingTransaction)?;
        Ok(tx
            .output
            .get(outpoint.vout as usize)
            .ok_or_else(|| Error::MissingVout)?
            .clone())
    }

    fn script_info(
        &self,
        script: &Script,
    ) -> Result<(DerivationPath, Vec<elements::bitcoin::PublicKey>), Error> {
        let index = self
            .store
            .cache
            .paths
            .get(script)
            .ok_or_else(|| Error::Generic(format!("{script} isn't in cache")))?;
        let derivation_path = vec![*index].into();
        let index = match index {
            ChildNumber::Normal { index } => *index,
            ChildNumber::Hardened { index: _ } => {
                return Err(Error::Generic("unexpected hardened derivation".into()))
            }
        };

        Ok((
            derivation_path,
            derive_public_keys(&self.descriptor, index)?,
        ))
    }

    fn validate_address(&self, address: &str) -> Result<Address, Error> {
        let params = self.config.address_params();
        let address = Address::parse_with_params(address, params)?;
        if address.blinding_pubkey.is_none() {
            return Err(Error::NotConfidentialAddress);
        };
        Ok(address)
    }

    fn validate_asset(&self, asset: &str) -> Result<AssetId, Error> {
        if asset.is_empty() {
            Ok(self.policy_asset())
        } else {
            Ok(AssetId::from_str(asset)?)
        }
    }

    fn validate_addressee(&self, addressee: &(u64, &str, &str)) -> Result<Addressee, Error> {
        let (satoshi, address, asset) = addressee;
        let address = self.validate_address(address)?;
        let asset = self.validate_asset(asset)?;
        Ok(Addressee {
            satoshi: *satoshi,
            address,
            asset,
        })
    }

    fn validate_addressees(
        &self,
        addressees: Vec<(u64, &str, &str)>,
    ) -> Result<Vec<Addressee>, Error> {
        addressees
            .iter()
            .map(|a| self.validate_addressee(a))
            .collect()
    }

    fn tot_out(&self, addressees: &Vec<Addressee>) -> Result<HashMap<AssetId, u64>, Error> {
        let mut r = HashMap::new();
        r.entry(self.policy_asset()).or_insert(0);
        for addressee in addressees {
            *r.entry(addressee.asset).or_default() += addressee.satoshi;
        }
        Ok(r)
    }

    fn add_output(
        &self,
        pset: &mut PartiallySignedTransaction,
        addressee: Addressee,
    ) -> Result<(), Error> {
        let output = Output {
            script_pubkey: addressee.address.script_pubkey(),
            amount: Some(addressee.satoshi),
            asset: Some(addressee.asset),
            blinding_key: addressee.address.blinding_pubkey.map(convert_pubkey),
            blinder_index: Some(0),
            ..Default::default()
        };
        pset.add_output(output);
        Ok(())
    }

    fn createpset(
        &mut self,
        addressees: Vec<(u64, &str, &str)>,
        fee: Option<u64>,
    ) -> Result<PartiallySignedTransaction, Error> {
        // Check user inputs
        let addressees = self.validate_addressees(addressees)?;

        // Get utxos
        let utxos = self.utxos()?;

        // Set fee
        let fee = fee.unwrap_or(1_000);

        // Check if we have enough funds and compute change
        let tot_in = self.balance_from_utxos(&utxos)?;
        let mut tot_out = self.tot_out(&addressees)?;
        *tot_out.entry(self.policy_asset()).or_default() += fee;
        let mut addressees_change = vec![];
        for (asset, satoshi_out) in tot_out.clone() {
            let satoshi_in: u64 = *tot_in.get(&asset).unwrap_or(&0);
            if satoshi_in < satoshi_out {
                return Err(Error::InsufficientFunds);
            }
            let satoshi_change = satoshi_in - satoshi_out;
            if satoshi_change > 0 {
                let address_change = self.address()?;
                addressees_change.push(Addressee {
                    satoshi: satoshi_change,
                    address: address_change,
                    asset,
                });
            }
        }

        // Init PSET
        let mut pset = PartiallySignedTransaction::new_v2();
        let mut inp_txout_sec = HashMap::new();

        // Add inputs
        for (idx, utxo) in utxos.iter().enumerate() {
            if tot_out.get(&utxo.unblinded.asset).is_none() {
                // Do not add utxos if the we are not sending the asset
                continue;
            }
            let mut input = Input::from_prevout(utxo.txo.outpoint);
            input.witness_utxo = Some(self.get_txout(&utxo.txo.outpoint)?);

            // TODO: fill more fields

            let (derivation_path, public_keys) = self.script_info(&utxo.txo.script_pubkey)?;
            for public_key in public_keys {
                // FIXME fingerprint
                input.bip32_derivation.insert(
                    public_key,
                    (Fingerprint::default(), derivation_path.clone()),
                );
            }

            pset.add_input(input);
            inp_txout_sec.insert(idx, utxo.unblinded);
        }

        // Add outputs
        for addressee in addressees {
            self.add_output(&mut pset, addressee)?;
        }
        for addressee in addressees_change {
            self.add_output(&mut pset, addressee)?;
        }
        let fee_output = Output::new_explicit(Script::default(), fee, self.policy_asset(), None);
        pset.add_output(fee_output);

        // Blind the transaction
        let mut rng = thread_rng();
        pset.blind_last(&mut rng, &EC, &inp_txout_sec)?;
        Ok(pset)
    }

    /// Create a PSET sending some satoshi to an address
    pub fn sendlbtc(
        &mut self,
        satoshi: u64,
        address: &str,
    ) -> Result<PartiallySignedTransaction, Error> {
        let addressees = vec![(satoshi, address, "")];
        self.createpset(addressees, None)
    }

    /// Create a PSET sending some satoshi of an asset to an address
    pub fn sendasset(
        &mut self,
        satoshi: u64,
        address: &str,
        asset: &str,
    ) -> Result<PartiallySignedTransaction, Error> {
        let addressees = vec![(satoshi, address, asset)];
        self.createpset(addressees, None)
    }

    /// Create a PSET sending to many outputs
    pub fn sendmany(
        &mut self,
        addressees: Vec<(u64, &str, &str)>,
    ) -> Result<PartiallySignedTransaction, Error> {
        self.createpset(addressees, None)
    }

    /// Create a PSET issuing an asset
    pub fn issueasset(
        &mut self,
        satoshi_asset: u64,
        satoshi_token: u64,
    ) -> Result<PartiallySignedTransaction, Error> {
        // Get utxos
        let utxos = self.asset_utxos(&self.policy_asset())?;
        let utxo = utxos[0].clone();

        // Set fee
        let fee = 1_000;

        // Init PSET
        let mut pset = PartiallySignedTransaction::new_v2();
        let mut inp_txout_sec = HashMap::new();

        // Add a policy asset input
        let mut input = Input::from_prevout(utxo.txo.outpoint);
        input.witness_utxo = Some(self.get_txout(&utxo.txo.outpoint)?);

        let (derivation_path, public_keys) = self.script_info(&utxo.txo.script_pubkey)?;
        for public_key in public_keys {
            input.bip32_derivation.insert(
                public_key,
                (Fingerprint::default(), derivation_path.clone()),
            );
        }

        // Set issuance data
        input.issuance_value_amount = Some(satoshi_asset);
        if satoshi_token > 0 {
            input.issuance_inflation_keys = Some(satoshi_token);
        }
        let prevout = OutPoint::new(input.previous_txid, input.previous_output_index);
        let contract_hash = elements::issuance::ContractHash::from_slice(&[0u8; 32]).unwrap();
        let asset_entropy =
            Some(AssetId::generate_asset_entropy(prevout, contract_hash).to_byte_array());
        input.issuance_asset_entropy = asset_entropy;
        let (asset, token) = input.issuance_ids();

        pset.add_input(input);
        let idx = 0;
        inp_txout_sec.insert(idx, utxo.unblinded);
        let satoshi_change = utxo.unblinded.value - fee;

        // Add outputs
        let mut addressees = vec![];
        addressees.push(Addressee {
            satoshi: satoshi_asset,
            address: self.address()?,
            asset,
        });
        if satoshi_token > 0 {
            addressees.push(Addressee {
                satoshi: satoshi_token,
                address: self.address()?,
                asset: token,
            });
        }
        addressees.push(Addressee {
            satoshi: satoshi_change,
            address: self.address()?,
            asset: self.policy_asset(),
        });

        for addressee in addressees {
            self.add_output(&mut pset, addressee)?;
        }
        let fee_output = Output::new_explicit(Script::default(), fee, self.policy_asset(), None);
        pset.add_output(fee_output);

        // Blind the transaction
        let mut rng = thread_rng();
        pset.blind_last(&mut rng, &EC, &inp_txout_sec)?;
        Ok(pset)
    }

    pub fn finalize(&self, pset: &mut PartiallySignedTransaction) -> Result<Transaction, Error> {
        // genesis_hash is only used for BIP341 (taproot) sighash computation
        elements_miniscript::psbt::finalize(pset, &EC, BlockHash::all_zeros()).unwrap();
        Ok(pset.extract_tx()?)
    }

    pub fn broadcast(&self, tx: &Transaction) -> Result<Txid, Error> {
        let client = self.config.electrum_url().build_client()?;
        let txid = client.transaction_broadcast_raw(&elements::encode::serialize(tx))?;
        Ok(Txid::from_raw_hash(txid.to_raw_hash()))
    }
}

fn convert_pubkey(pk: elements::secp256k1_zkp::PublicKey) -> elements::bitcoin::key::PublicKey {
    elements::bitcoin::key::PublicKey::new(pk)
}

#[cfg(test)]
mod tests {
    use super::*;
    use elements::bitcoin::bip32::{ExtendedPrivKey, ExtendedPubKey};
    use elements::bitcoin::network::constants::Network;
    use elements::encode::Encodable;
    use elements::secp256k1_zkp::Scalar;
    use elements_miniscript::confidential::bare::TweakHash;
    use elements_miniscript::confidential::Key;
    use elements_miniscript::descriptor::DescriptorSecretKey;
    use elements_miniscript::elements::AddressParams;
    use elements_miniscript::{ConfidentialDescriptor, DefiniteDescriptorKey, DescriptorPublicKey};
    use std::str::FromStr;

    #[test]
    fn test_desc() {
        let xpub = "tpubDD7tXK8KeQ3YY83yWq755fHY2JW8Ha8Q765tknUM5rSvjPcGWfUppDFMpQ1ScziKfW3ZNtZvAD7M3u7bSs7HofjTD3KP3YxPK7X6hwV8Rk2";
        let master_blinding_key =
            "9c8e4f05c7711a98c838be228bcb84924d4570ca53f35fa1c793e58841d47023";
        let checksum = "qw2qy2ml";
        let desc_str = format!(
            "ct(slip77({}),elwpkh({}))#{}",
            master_blinding_key, xpub, checksum
        );
        let desc = ConfidentialDescriptor::<DefiniteDescriptorKey>::from_str(&desc_str).unwrap();
        let addr = desc.address(&EC, &AddressParams::ELEMENTS).unwrap();
        let expected_addr = "el1qqthj9zn320epzlcgd07kktp5ae2xgx82fkm42qqxaqg80l0fszueszj4mdsceqqfpv24x0cmkvd8awux8agrc32m9nj9sp0hk";
        assert_eq!(addr.to_string(), expected_addr.to_string());
    }

    #[test]
    fn test_address_from_desc_wildcard() {
        let xpub = "tpubDC2Q4xK4XH72GLdvD62W5NsFiD3HmTScXpopTsf3b4AUqkQwBd7wmWAJki61sov1MVuyU4MuGLJHF7h3j1b3e1FY2wvUVVx7vagmxdPvVsv";
        let master_blinding_key =
            "9c8e4f05c7711a98c838be228bcb84924d4570ca53f35fa1c793e58841d47023";
        let checksum = "yfhwtmd8";
        let desc_str = format!(
            "ct(slip77({}),elsh(wpkh({}/0/*)))#{}",
            master_blinding_key, xpub, checksum
        );
        let desc = ConfidentialDescriptor::<DescriptorPublicKey>::from_str(&desc_str).unwrap();

        let addr = derive_address(&desc, 0, &AddressParams::LIQUID_TESTNET).unwrap();
        let expected_addr =
            "vjTwLVioiKrDJ7zZZn9iQQrxP6RPpcvpHBhzZrbdZKKVZE29FuXSnkXdKcxK3qD5t1rYsdxcm9KYRMji";
        assert_eq!(addr.to_string(), expected_addr.to_string());

        let addr = derive_address(&desc, 1, &AddressParams::LIQUID_TESTNET).unwrap();
        let expected_addr =
            "vjTuhaPWWbywbSy2EeRWWQ8bN2pPLmM4gFQTkA7DPX7uaCApKuav1e6LW1GKHuLUHdbv9Eag5MybsZoy";
        assert_eq!(addr.to_string(), expected_addr.to_string());
    }

    #[test]
    fn test_blinding_private() {
        // Get a confidential address from a "view" descriptor
        let seed = [0u8; 16];
        let xprv = ExtendedPrivKey::new_master(Network::Regtest, &seed).unwrap();
        let xpub = ExtendedPubKey::from_priv(&EC, &xprv);
        let checksum = "h0ej28gv";
        let desc_str = format!("ct({},elwpkh({}))#{}", xprv, xpub, checksum);
        println!("{}", desc_str);
        let desc = ConfidentialDescriptor::<DefiniteDescriptorKey>::from_str(&desc_str).unwrap();
        let address = desc.address(&EC, &AddressParams::ELEMENTS).unwrap();
        // and extract the public blinding key
        let pk_from_addr = address.blinding_pubkey.unwrap();

        // Get the public blinding key from the descriptor blinding key
        let key = match desc.key {
            Key::View(DescriptorSecretKey::XPrv(dxk)) => dxk.xkey.to_priv(),
            _ => todo!(),
        };
        // tweaked_private_key needs fixes upstream
        let mut eng = TweakHash::engine();
        key.public_key(&EC)
            .write_into(&mut eng)
            .expect("engines don't error");
        address
            .script_pubkey()
            .consensus_encode(&mut eng)
            .expect("engines don't error");
        let hash_bytes = TweakHash::from_engine(eng).to_byte_array();
        let hash_scalar = Scalar::from_be_bytes(hash_bytes).expect("bytes from hash");
        let tweaked_key = key.inner.add_tweak(&hash_scalar).unwrap();
        let pk_from_view = tweaked_key.public_key(&EC);

        assert_eq!(pk_from_addr, pk_from_view);
    }

    #[test]
    fn test_view_single() {
        // elements_miniscript does not support single "view" blinding keys and exteneded keys in
        // the "bitcoin" descriptor"
        let descriptor_blinding_key =
            "9c8e4f05c7711a98c838be228bcb84924d4570ca53f35fa1c793e58841d47023";
        let xpub = "tpubDD7tXK8KeQ3YY83yWq755fHY2JW8Ha8Q765tknUM5rSvjPcGWfUppDFMpQ1ScziKfW3ZNtZvAD7M3u7bSs7HofjTD3KP3YxPK7X6hwV8Rk2";
        let checksum = "scscm6zj";
        let desc_str = format!(
            "ct({},elwpkh({}))#{}",
            descriptor_blinding_key, xpub, checksum
        );
        let expected_err =
            elements_miniscript::Error::Unexpected("Error while parsing xkey.".into());
        let err = ConfidentialDescriptor::<DefiniteDescriptorKey>::from_str(&desc_str).unwrap_err();
        assert_eq!(err, expected_err);
        let err = ConfidentialDescriptor::<DescriptorPublicKey>::from_str(&desc_str).unwrap_err();
        assert_eq!(err, expected_err);
    }
}
