use age::x25519::Recipient;
use elements::{BlockHash, Script, Txid};
use std::collections::{HashMap, HashSet};
use tokio::runtime::Runtime;

use crate::{
    clients::{asyncr, Capability, Data, History},
    store::Height,
    wollet::WolletState,
    BlockchainBackend, ElementsNetwork, Error, WolletDescriptor,
};

#[derive(Debug)]
/// A blockchain backend implementation based on the
/// [esplora HTTP API](https://github.com/blockstream/esplora/blob/master/API.md)
pub struct EsploraClient {
    rt: Runtime,
    client: asyncr::EsploraClient,
}

impl EsploraClient {
    pub fn new(url: &str, network: ElementsNetwork) -> Result<Self, Error> {
        Ok(Self {
            rt: Runtime::new()?,
            client: asyncr::EsploraClient::new(network, url, false),
        })
    }
}

/// "Waterfalls" methods
impl EsploraClient {
    /// Create a new Esplora client using the "waterfalls" endpoint
    pub fn new_waterfalls(url: &str, network: ElementsNetwork) -> Result<Self, Error> {
        Ok(Self {
            rt: Runtime::new()?,
            client: asyncr::EsploraClient::new(network, url, true),
        })
    }

    /// Do not encrypt the descriptor when using the "waterfalls" endpoint
    pub fn waterfalls_avoid_encryption(&mut self) {
        self.client.waterfalls_avoid_encryption = true;
    }

    pub fn waterfalls_server_recipient(&mut self) -> Result<Recipient, Error> {
        self.rt.block_on(self.client.waterfalls_server_recipient())
    }
}

impl BlockchainBackend for EsploraClient {
    fn tip(&mut self) -> Result<elements::BlockHeader, crate::Error> {
        self.rt.block_on(self.client.tip())
    }

    fn broadcast(&self, tx: &elements::Transaction) -> Result<elements::Txid, crate::Error> {
        self.rt.block_on(self.client.broadcast(tx))
    }

    fn get_transactions(&self, txids: &[Txid]) -> Result<Vec<elements::Transaction>, Error> {
        self.rt.block_on(self.client.get_transactions(txids))
    }

    fn get_headers(
        &self,
        heights: &[Height],
        height_blockhash: &HashMap<Height, BlockHash>,
    ) -> Result<Vec<elements::BlockHeader>, Error> {
        self.rt
            .block_on(self.client.get_headers(heights, height_blockhash))
    }

    // examples:
    // https://blockstream.info/liquidtestnet/api/address/tex1qntw9m0j2e93n84x975t47ddhgkzx3x8lhfv2nj/txs
    // https://blockstream.info/liquidtestnet/api/scripthash/b50a2a798d876db54acfa0d8dfdc49154ea8defed37b225ec4c9ec7415358ba3/txs
    fn get_scripts_history(&self, scripts: &[&Script]) -> Result<Vec<Vec<History>>, Error> {
        self.rt.block_on(self.client.get_scripts_history(scripts))
    }

    fn capabilities(&self) -> HashSet<Capability> {
        self.client.capabilities()
    }

    fn get_history_waterfalls<S: WolletState>(
        &mut self,
        descriptor: &WolletDescriptor,
        state: &S,
    ) -> Result<Data, Error> {
        self.rt
            .block_on(self.client.get_history_waterfalls(descriptor, state))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::EsploraClient;
    use crate::{BlockchainBackend, ElementsNetwork};
    use elements::{encode::Decodable, BlockHash};

    fn get_block(base_url: &str, hash: BlockHash) -> elements::Block {
        let url = format!("{}/block/{}/raw", base_url, hash);
        let response = reqwest::blocking::get(url).unwrap();
        elements::Block::consensus_decode(&response.bytes().unwrap()[..]).unwrap()
    }

    #[ignore = "Should be integration test, but it is testing private function"]
    #[test]
    fn esplora_local() {
        let server = lwk_test_util::setup_with_esplora();

        let esplora_url = format!("http://{}", server.electrs.esplora_url.as_ref().unwrap());
        test_esplora_url(&esplora_url);
    }

    #[ignore]
    #[test]
    fn esplora_testnet() {
        test_esplora_url("https://blockstream.info/liquidtestnet/api");
        test_esplora_url("https://liquid.network/liquidtestnet/api");
    }

    fn test_esplora_url(esplora_url: &str) {
        println!("{}", esplora_url);

        let mut client =
            EsploraClient::new(esplora_url, ElementsNetwork::default_regtest()).unwrap();
        let header = client.tip().unwrap();
        assert!(header.height > 100);

        let headers = client.get_headers(&[0], &HashMap::new()).unwrap();
        let genesis_header = &headers[0];
        assert_eq!(genesis_header.height, 0);

        let genesis_block = get_block(esplora_url, genesis_header.block_hash());
        let genesis_tx = &genesis_block.txdata[0];

        let txid = genesis_tx.txid();
        let txs = client.get_transactions(&[txid]).unwrap();

        assert_eq!(txs[0].txid(), txid);

        let existing_script = &genesis_tx.output[0].script_pubkey;

        let histories = client.get_scripts_history(&[existing_script]).unwrap();
        assert!(!histories.is_empty())
    }
}
