/*
 * Example smart contract written in RUST
 *
 * Learn more about writing NEAR smart contracts with Rust:
 * https://near-docs.io/develop/Contract
 *
 */

use near_sdk::borsh::{self, BorshDeserialize, BorshSerialize};
use near_sdk::{
    log,
    near_bindgen,
    AccountId,
    Balance, 
    env,
    collections::{
        LookupMap,
    }
};
use barnett_smart_card_protocol::discrete_log_cards::{
    BnPublicKeyBuf,
    BnParamsBuf,
    BnMaskedCardBuf,
    BnRevealTokenWithProofBuf,
    BnZKProofKeyOwnershipBuf,
    BnPublicKey,
    BnPlayerSecretKey,
    BnCard,
    BnMaskedCard, 
    BnRevealToken, 
    BnZKProofShuffle,
    BnParameters, 
    BnCardProtocol,
    BnZKProofKeyOwnership,
    BnZKProofMasking,
    BnZKProofRemasking,
    BnZKProofReveal,
};


const GAMES_STORAGE_KEY: &'static [u8] = b"GAMES";

type GameId = [u8; 32];

// Define the contract structure
#[near_bindgen]
#[derive(BorshDeserialize, BorshSerialize)]
pub struct Contract {
    games: LookupMap<GameId, Game>,
    trusted_setup_params: BnParamsBuf,
}

#[derive(BorshDeserialize, BorshSerialize)]
pub struct GameLobby {
    pub id: String,
    pub player_account_ids: Vec<AccountId>,
    pub player_game_pubkeys: Vec<BnPublicKeyBuf>,
}

impl GameLobby {
    fn new(id: String, player_account_ids: Vec<AccountId>, player_game_pubkeys: Vec<BnPublicKeyBuf>) -> Self {
        Self {
            id,
            player_account_ids,
            player_game_pubkeys,
        }
    }

    fn creator(&self) -> AccountId {
        self.player_account_ids[0].clone()
    }

    fn num_players(&self) -> usize {
        self.player_account_ids.len()
    }

    fn add_player(&mut self, account_id: AccountId, game_pubkey: BnPublicKeyBuf) {
        self.player_account_ids.push(account_id);
        self.player_game_pubkeys.push(game_pubkey);
    }
}

#[derive(BorshDeserialize, BorshSerialize)]
pub struct GameState {
    pub id: String,
    pub player_account_ids: Vec<AccountId>,
    // game state

    /// the current player whose turn it is
    pub turn: usize,

    /// the current "dealer". Since everyone shuffles, in practice this is just the player who publishes the initial deck for the round
    /// this is also used to determine who the little blind, big blind, and action is
    pub dealer: usize,

    /// the amounts each player has bet so far
    pub bets: Vec<Balance>,

    /// the number of "chips" each player has
    pub balances: Vec<Balance>,

    // cryptography state

    /// public parameters for the protocol
    /// generated via a trusted setup (done once upon deploying the contract - they just get copied from game to game)
    pub pp: BnParamsBuf,

    /// the game public keys for each player (not to be confused with their near account public keys)
    pub player_game_pubkeys: Vec<BnPublicKeyBuf>,

    /// the aggregate public key for the protocol
    pub aggregate_pubkey: BnPublicKeyBuf,

    /// the masked / shuffled deck. This is updated only during the shuffle phase at the beginning of each round
    pub deck: Vec<BnMaskedCardBuf>,

    /// "reveal tokens" revealed for each card, by each player.
    /// i.e. reveal_tokens_with_proofs[card_index][player_index] is the reveal token for card_index, revealed by player_index, if they have provided it
    /// a player can only unmask a card once all of the reveal tokens for that card have been received
    pub reveal_tokens_with_proofs: Vec<Vec<Option<BnRevealTokenWithProofBuf>>>,
}

#[derive(BorshDeserialize, BorshSerialize)]
pub enum Game {
    WaitingForPlayers(GameLobby),
    InProgress(GameState),
    Finished,
}

// Implement the contract structure
#[near_bindgen]
impl Contract {
    #[init]
    #[private]
    pub fn init(trusted_setup_params: BnParamsBuf) -> Self {
        Self {
            games: LookupMap::new(GAMES_STORAGE_KEY),
            trusted_setup_params,
        }
    }

    // pub fn create_game(&mut self, id: GameId, creator_pk: BnPublicKeyBuf, creator_key_ownership_proof: BnZKProofKeyOwnershipProofBuf) {
    //     let pk = creator.deserialize().expect("failed to deserialize public key buf");
    //     let 

    //     let creator_account_id = env::predecessor_account_id();
    //     let lobby = GameLobby::new(id, vec![creator_account_id], vec![creator_pk]);

    //     self.games.insert(&id, &Game::WaitingForPlayers(lobby));
    // }
}

/*
 * The rest of this file holds the inline tests for the code above
 * Learn more about Rust tests: https://doc.rust-lang.org/book/ch11-01-writing-tests.html
 */
#[cfg(test)]
mod tests {
    use super::*;

    // #[test]
    // fn get_default_greeting() {
    //     let contract = Contract::default();
    //     // this test did not call set_greeting so should return the default "Hello" greeting
    //     assert_eq!(
    //         contract.get_greeting(),
    //         "Hello".to_string()
    //     );
    // }

    // #[test]
    // fn set_then_get_greeting() {
    //     let mut contract = Contract::default();
    //     contract.set_greeting("howdy".to_string());
    //     assert_eq!(
    //         contract.get_greeting(),
    //         "howdy".to_string()
    //     );
    // }
}
