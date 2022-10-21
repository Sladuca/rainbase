use std::{env, fs};
use near_units::parse_near;
use serde_json::json;
use workspaces::{Account, Contract};
use rand::{Rng, thread_rng};
use ark_ff::One;
use ark_ec::ProjectiveCurve;
use barnett_smart_card_protocol::{
	BarnettSmartProtocol,
	discrete_log_cards::{
		BnParameters,
		BnParamsBuf,
		BnCardProtocol, BnPublicKeyBuf, BnPlayerSecretKey, BnPlayerSecretKeyBuf, BnZKProofKeyOwnershipBuf, get_card_elems_buf, BnScalar, BnMaskedCardBuf
	}
};

const NUM_PLAYERS: usize = 4;
const M: usize = 2;
const N: usize = 26;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let wasm_arg: &str = &(env::args().nth(1).unwrap());
    let wasm_filepath = fs::canonicalize(env::current_dir()?.join(wasm_arg))?;

    let worker = workspaces::sandbox().await?;
    let wasm = std::fs::read(wasm_filepath)?;
    let contract = worker.dev_deploy(&wasm).await?;
   
    // do trusted setup
	let mut rng = thread_rng();
	let params = BnCardProtocol::setup(&mut rng, M, N).unwrap();
    let params_buf = BnParamsBuf::serialize(params.clone()).unwrap();
    contract.call("init")
        .gas(near_units::parse_gas!("300 T") as u64)
        .args_json(json!({ "trusted_setup_params": params_buf }))
        .transact()
        .await?
        .into_result()?;

    // create accounts
    let mut players = Vec::new();
    for _ in 0..NUM_PLAYERS {
        let account = worker.dev_create_account().await?;
        players.push(account);
    }

    // begin tests
    test_one_round(&players, &contract, &params).await?;
    Ok(())
}

async fn test_one_round(
    players: &[Account],
    contract: &Contract,
    params: &BnParameters,
) -> anyhow::Result<()> {
    let alice = &players[0];
    let bob = &players[1];
    let carol = &players[2];
    let dave = &players[3];

	let mut rng = thread_rng();

    // everyone generates game keys
    let (alice_pk, alice_sk) = BnCardProtocol::player_keygen(&mut rng, params).unwrap();
    let (bob_pk, bob_sk) = BnCardProtocol::player_keygen(&mut rng, params).unwrap();
    let (carol_pk, carol_sk) = BnCardProtocol::player_keygen(&mut rng, params).unwrap();
    let (dave_pk, dave_sk) = BnCardProtocol::player_keygen(&mut rng, params).unwrap();

    let alice_pk_buf = BnPublicKeyBuf::serialize(alice_pk).unwrap();
    let alice_sk_buf = BnPlayerSecretKeyBuf::serialize(alice_sk).unwrap();
    let bob_pk_buf = BnPublicKeyBuf::serialize(bob_pk).unwrap();
    let bob_sk_buf = BnPlayerSecretKeyBuf::serialize(bob_sk).unwrap();
    let carol_pk_buf = BnPublicKeyBuf::serialize(carol_pk).unwrap();
    let carol_sk_buf = BnPlayerSecretKeyBuf::serialize(carol_sk).unwrap();
    let dave_pk_buf = BnPublicKeyBuf::serialize(dave_pk).unwrap();
    let dave_sk_buf = BnPlayerSecretKeyBuf::serialize(dave_sk).unwrap();


    // everyone generates key ownership proofs
    let alice_key_proof = BnCardProtocol::prove_key_ownership(&mut rng, params, &alice_pk, &alice_sk, alice.id().as_bytes()).unwrap();
    let bob_key_proof = BnCardProtocol::prove_key_ownership(&mut rng, params, &bob_pk, &bob_sk, bob.id().as_bytes()).unwrap();
    let carol_key_proof = BnCardProtocol::prove_key_ownership(&mut rng, params, &carol_pk, &carol_sk, carol.id().as_bytes()).unwrap();
    let dave_key_proof = BnCardProtocol::prove_key_ownership(&mut rng, params, &dave_pk, &dave_sk, dave.id().as_bytes()).unwrap();

    let alice_key_proof_buf = BnZKProofKeyOwnershipBuf::serialize(alice_key_proof).unwrap();
    let bob_key_proof_buf = BnZKProofKeyOwnershipBuf::serialize(bob_key_proof).unwrap();
    let carol_key_proof_buf = BnZKProofKeyOwnershipBuf::serialize(carol_key_proof).unwrap();
    let dave_key_proof_buf = BnZKProofKeyOwnershipBuf::serialize(dave_key_proof).unwrap();

    let _alice_pk = alice_pk_buf.deserialize().unwrap();
    let _alice_proof = alice_key_proof_buf.deserialize().unwrap();

    let params_buf = BnParamsBuf::serialize(params.clone()).unwrap();
    let _pp = params_buf.deserialize().unwrap();

    // alice creates a game
    let game_id: [u8; 4] = alice.call(contract.id(), "create_game")
        .gas(near_units::parse_gas!("300 T") as u64)
        .args_json(json!({
            "creator_pk": alice_pk_buf,
            "creator_key_ownership_proof": alice_key_proof_buf,
        }))
        .transact()
        .await?
        .json()?;

    // // bob joins the game
    // bob.call(contract.id(), "join_game")
    //     .gas(near_units::parse_gas!("300 T") as u64)
    //     .args_json(json!({
    //         "game_id": game_id,
    //         "pk": bob_pk_buf,
    //         "key_ownership_proof": bob_key_proof_buf,
    //     }))
    //     .transact()
    //     .await?
    //     .into_result()?;

    // // carol joins the game
    // carol.call(contract.id(), "join_game")
    //     .gas(near_units::parse_gas!("300 T") as u64)
    //     .args_json(json!({
    //         "game_id": game_id,
    //         "pk": carol_pk_buf,
    //         "key_ownership_proof": carol_key_proof_buf,
    //     }))
    //     .transact()
    //     .await?
    //     .into_result()?;
    
    // // dave joins the game
    // dave.call(contract.id(), "join_game")
    //     .gas(near_units::parse_gas!("300 T") as u64)
    //     .args_json(json!({
    //         "game_id": game_id,
    //         "pk": dave_pk_buf,
    //         "key_ownership_proof": dave_key_proof_buf,
    //     }))
    //     .transact()
    //     .await?
    //     .into_result()?;
  
    // // alice starts the game
    // alice.call(contract.id(), "start_game")
    //     .gas(near_units::parse_gas!("300 T") as u64)
    //     .args_json(json!({
    //         "game_id": game_id,
    //     }))
    //     .transact()
    //     .await?
    //     .into_result()?;
    
    // // get the aggregate key
    // let agg_pk_buf: BnPublicKeyBuf = alice.call(contract.id(), "get_aggregate_pk")
    //     .gas(near_units::parse_gas!("300 T") as u64)
    //     .args_json(json!({
    //         "game_id": game_id,
    //     }))
    //     .transact()
    //     .await?
    //     .json()?;
    // let agg_pk = BnPublicKeyBuf::deserialize(&agg_pk_buf).unwrap();
    
    // // alice inits the deck
    // // ignore the masking proofs for now, it's too much gas
    // let cards_buf = get_card_elems_buf(52).unwrap();
    // let cards = cards_buf.into_iter().map(|c| c.deserialize().unwrap()).map(|c| {
    //     BnCardProtocol::mask(&mut rng, params, &agg_pk, &c, &BnScalar::one()).unwrap()
    // }).map(|(card, _proof)| card).collect::<Vec<_>>();
    // let cards_buf = cards.iter().cloned().map(|c| BnMaskedCardBuf::serialize(c).unwrap()).collect::<Vec<_>>();

    // alice.call(contract.id(), "init_deck")
    //     .gas(near_units::parse_gas!("300 T") as u64)
    //     .args_json(json!({
    //         "game_id": game_id,
    //         "cards": cards_buf,
    //     }))
    //     .transact()
    //     .await?
    //     .into_result()?;

    Ok(())
}

async fn test_default_message(
    user: &Account,
    contract: &Contract,
) -> anyhow::Result<()> {
    let message: String = user
        .call( contract.id(), "get_greeting")
        .args_json(json!({}))
        .transact()
        .await?
        .json()?;

    assert_eq!(message, "Hello".to_string());
    println!("      Passed ✅ gets default message");
    Ok(())
}

async fn test_changes_message(
    user: &Account,
    contract: &Contract,
) -> anyhow::Result<()> {
    user.call(contract.id(), "set_greeting")
        .args_json(json!({"message": "Howdy"}))
        .transact()
        .await?
        .into_result()?;

    let message: String = user
        .call(contract.id(), "get_greeting")
        .args_json(json!({}))
        .transact()
        .await?
        .json()?;

    assert_eq!(message, "Howdy".to_string());
    println!("      Passed ✅ changes message");
    Ok(())
}