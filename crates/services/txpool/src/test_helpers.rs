// Rust isn't smart enough to detect cross module test deps
#![allow(dead_code)]

use crate::MockDb;
use fuel_core_types::{
    entities::coins::coin::{
        Coin,
        CompressedCoin,
    },
    fuel_asm::op,
    fuel_crypto::rand::{
        rngs::StdRng,
        Rng,
    },
    fuel_tx::{
        input::{
            coin::{
                CoinPredicate,
                CoinSigned,
            },
            contract::Contract,
        },
        Input,
        Output,
        UtxoId,
    },
    fuel_types::{
        AssetId,
        Word,
    },
};

// use some arbitrary large amount, this shouldn't affect the txpool logic except for covering
// the byte and gas price fees.
pub const TEST_COIN_AMOUNT: u64 = 100_000_000u64;

pub(crate) fn setup_coin(rng: &mut StdRng, mock_db: Option<&MockDb>) -> (Coin, Input) {
    let input = random_predicate(rng, AssetId::BASE, TEST_COIN_AMOUNT, None);
    add_coin_to_state(input, mock_db)
}

pub(crate) fn add_coin_to_state(input: Input, mock_db: Option<&MockDb>) -> (Coin, Input) {
    let coin = CompressedCoin {
        owner: *input.input_owner().unwrap(),
        amount: TEST_COIN_AMOUNT,
        asset_id: *input.asset_id().unwrap(),
        maturity: Default::default(),
        tx_pointer: Default::default(),
    };
    let utxo_id = *input.utxo_id().unwrap();
    if let Some(mock_db) = mock_db {
        mock_db
            .data
            .lock()
            .unwrap()
            .coins
            .insert(utxo_id, coin.clone());
    }
    (coin.uncompress(utxo_id), input)
}

pub(crate) fn create_output_and_input(
    rng: &mut StdRng,
    amount: Word,
) -> (Output, UnsetInput) {
    let input = random_predicate(rng, AssetId::BASE, amount, None);
    let output = Output::coin(*input.input_owner().unwrap(), amount, AssetId::BASE);
    (output, UnsetInput(input))
}

pub struct UnsetInput(Input);

impl UnsetInput {
    pub fn into_input(self, new_utxo_id: UtxoId) -> Input {
        let mut input = self.0;
        match &mut input {
            Input::CoinSigned(CoinSigned { utxo_id, .. })
            | Input::CoinPredicate(CoinPredicate { utxo_id, .. })
            | Input::Contract(Contract { utxo_id, .. }) => {
                *utxo_id = new_utxo_id;
            }
            _ => {}
        }
        input
    }
}

pub(crate) fn random_predicate(
    rng: &mut StdRng,
    asset_id: AssetId,
    amount: Word,
    utxo_id: Option<UtxoId>,
) -> Input {
    // use predicate inputs to avoid expensive cryptography for signatures
    let mut predicate_code: Vec<u8> = vec![op::ret(1)].into_iter().collect();
    // append some randomizing bytes after the predicate has already returned.
    predicate_code.push(rng.gen());
    let owner = Input::predicate_owner(&predicate_code);
    Input::coin_predicate(
        utxo_id.unwrap_or_else(|| rng.gen()),
        owner,
        amount,
        asset_id,
        Default::default(),
        0,
        predicate_code,
        vec![],
    )
}

pub(crate) fn custom_predicate(
    rng: &mut StdRng,
    asset_id: AssetId,
    amount: Word,
    code: Vec<u8>,
    utxo_id: Option<UtxoId>,
) -> Input {
    let owner = Input::predicate_owner(&code);
    Input::coin_predicate(
        utxo_id.unwrap_or_else(|| rng.gen()),
        owner,
        amount,
        asset_id,
        Default::default(),
        0,
        code,
        vec![],
    )
}
