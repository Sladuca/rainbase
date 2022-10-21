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
use barnett_smart_card_protocol::{
    BarnettSmartProtocol,
    discrete_log_cards::{
        BnPublicKeyBuf,
        BnParamsBuf,
        BnMaskedCardBuf,
        BnRevealTokenWithProofBuf,
        BnZKProofKeyOwnershipBuf,
        BnCardBuf,
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
        get_card_elems_buf,
    }
};
use rand::{
    Rng,
    SeedableRng,
    rngs::StdRng
};

const GAMES_STORAGE_KEY: &'static [u8] = b"GAMES";

type GameId = [u8; 4];

// Define the contract structure
#[near_bindgen]
#[derive(BorshDeserialize, BorshSerialize)]
pub struct Contract {
    games: LookupMap<GameId, Game>,
    trusted_setup_params: BnParamsBuf,
    card_values: Vec<BnCardBuf>,
}

#[derive(BorshDeserialize, BorshSerialize)]
pub struct GameLobby {
    /// the id for the game. players will use this to join the game
    pub id: GameId,

    /// the NEAR AccountIds of the players in the game
    /// the 0th playre
    pub player_account_ids: Vec<AccountId>,
    pub player_game_pubkeys: Vec<BnPublicKeyBuf>,

    /// this is used to detect stale lobbies. Lobbies more than 30 minutes old will be deleted
    pub created_at: u64,
}

impl GameLobby {
    fn new(id: GameId, player_account_ids: Vec<AccountId>, player_game_pubkeys: Vec<BnPublicKeyBuf>) -> Self {
        let created_at = env::block_timestamp();
        Self {
            id,
            player_account_ids,
            player_game_pubkeys,
            created_at,
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
    pub id: GameId,
    pub player_account_ids: Vec<AccountId>,

    // game state

    /// the current phase of the round - can be one of INIT, SHUFFLE, BET0, FLOP, BET1, TURN, BET2, RIVER, BET3, SHOWDOWN
    pub phase: Phase,

    /// the current player whose turn it is
    pub turn: usize,

    /// the current "dealer". Since everyone shuffles, in practice this is just the player who publishes the initial deck for the round
    /// this is also used to determine who the little blind, big blind, and action is
    /// this increases by one each round
    pub dealer: usize,

    /// the amounts each player has bet so far
    pub bets: Vec<Balance>,

    /// which players are still in on the round
    /// when a player folds, their corresponding `Option` is set to `None`
    pub players_in: Vec<Option<()>>,

    /// the number of "chips" each player has
    pub balances: Vec<Balance>,

    // TODO: find a more intelligent way to do this
    /// used to detect when the game is "over". Games that haven't been touched for over an hour are considered "over"
    pub last_modified: u64,

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

impl GameState {
    fn new(id: GameId, player_account_ids: Vec<AccountId>, player_game_pubkeys: Vec<BnPublicKeyBuf>, pp: BnParamsBuf) -> Self {
        let num_players = player_account_ids.len();
        let _pp = pp.deserialize().expect("failed to deserialize public parameters");
        let mut player_infos = Vec::new();
        for (account_id, pk) in player_account_ids.iter().zip(player_game_pubkeys.iter()) {
            let pk = pk.deserialize().expect("failed to deserialize player public key");
            player_infos.push((pk, account_id.as_bytes()));
        }

        let aggregate_pubkey = BnCardProtocol::compute_aggregate_key(&_pp, &player_infos, None).expect("failed to aggregate public keys");
        let aggregate_pubkey = BnPublicKeyBuf::serialize(aggregate_pubkey).expect("failed to serialize aggregate public key");
        
        Self {
            id,
            player_account_ids,
            phase: Phase::INIT,
            turn: 0,
            dealer: 0,
            bets: vec![0; num_players],
            players_in: vec![Some(()); num_players],
            balances: vec![0; num_players],
            last_modified: env::block_timestamp(),
            pp,
            player_game_pubkeys,
            aggregate_pubkey,
            deck: vec![],
            reveal_tokens_with_proofs: vec![vec![None; num_players]; 52],
        }
    }

    fn num_players(&self) -> usize {
        self.player_account_ids.len()
    }

    fn num_cards(&self) -> usize {
        let pp = self.pp.deserialize().expect("failed to deserialize public parameters");
        pp.num_cards()
    }

    fn player_index(&self, account_id: &AccountId) -> Option<usize> {
        self.player_account_ids.iter().position(|id| id == account_id)
    }

    fn player_account_id(&self, player_index: usize) -> AccountId {
        self.player_account_ids[player_index].clone()
    }
}


#[derive(BorshDeserialize, BorshSerialize)]
pub enum Phase {
    INIT,
    SHUFFLE,
    BET0,
    FLOP,
    BET1,
    TURN,
    BET2,
    RIVER,
    BET3,
    SHOWDOWN,
}

#[derive(BorshDeserialize, BorshSerialize)]
pub enum Game {
    WaitingForPlayers(GameLobby),
    InProgress(GameState),
}


// temporary hack so it deploys without automating trusted setup
impl Default for Contract {
    fn default() -> Self {
        let card_values = get_card_elems_buf(52).unwrap();
        Self {
            games: LookupMap::new(GAMES_STORAGE_KEY),
            trusted_setup_params: BnParamsBuf { buf: vec![] },
            card_values,
        }
    }
}

// Implement the contract structure
#[near_bindgen]
impl Contract {
    // #[init]
    // #[private]
    // pub fn init(trusted_setup_params: BnParamsBuf) -> Self {
    //     let card_values = get_card_elems_buf(52).unwrap();
    //     Self {
    //         games: LookupMap::new(GAMES_STORAGE_KEY),
    //         trusted_setup_params,
    //         card_values,
    //     }
    // }

    fn generate_game_id(&self) -> GameId {
        let seed = env::random_seed();
        assert!(seed.len() >= 32, "random seed is too short - this should never happen!");

        let mut rng = StdRng::from_seed(seed[0..32].try_into().unwrap());

        // TODO: find a more intelligent way to do this
        loop {
            let digits: [u8; 4] = [(); 4].map(|_| rng.gen_range(0..10));
            if !self.games.contains_key(&digits) {
                return digits;
            }

            let game = self.games.get(&digits).unwrap();
            if let Game::InProgress(ref game_state) = game {
                if env::block_timestamp() - game_state.last_modified > 3600 * 1000 {
                    return digits;
                }
            }

            if let Game::WaitingForPlayers(ref lobby) = game {
                if env::block_timestamp() - lobby.created_at > 20 * 60 * 1000 {
                    return digits;
                }
            }
        }
    }

    pub fn create_game(&mut self, creator_pk: BnPublicKeyBuf, creator_key_ownership_proof: BnZKProofKeyOwnershipBuf) -> GameId {
        let pk = creator_pk.deserialize().expect("failed to deserialize public key");
        let proof = creator_key_ownership_proof.deserialize().expect("failed to deserialize key ownership proof");
        let pp = self.trusted_setup_params.deserialize().expect("failed to deserialize trusted setup params");
        let creator_account_id = env::predecessor_account_id();
        let creator_account_id_bytes = creator_account_id.as_bytes();

        BnCardProtocol::verify_key_ownership(&pp, &pk, &creator_account_id_bytes, &proof).expect("failed to verify key ownership proof");

        let game_id = self.generate_game_id();
        let lobby = GameLobby::new(game_id, vec![creator_account_id], vec![creator_pk]);

        self.games.insert(&game_id, &Game::WaitingForPlayers(lobby));
        game_id
    }

    pub fn join_game(&mut self, game_id: GameId, pk: BnPublicKeyBuf, key_ownership_proof: BnZKProofKeyOwnershipBuf) {
        assert!(self.games.contains_key(&game_id), "game does not exist");

        let mut game = self.games.get(&game_id).unwrap();

        match game {
            Game::WaitingForPlayers(ref mut lobby) => {
                let account_id = env::predecessor_account_id();
                assert!(lobby.player_account_ids.iter().all(|id| id != &account_id), "already in lobby");

                let _pk = pk.deserialize().expect("failed to deserialize public key");
                let proof = key_ownership_proof.deserialize().expect("failed to deserialize key ownership proof");
                let pp = self.trusted_setup_params.deserialize().expect("failed to deserialize trusted setup params");

                let account_id_bytes = account_id.as_bytes();
                BnCardProtocol::verify_key_ownership(&pp, &_pk, &account_id_bytes, &proof).expect("failed to verify key ownership proof");

                lobby.add_player(account_id, pk);
            },
            _ => panic!("game is no longer accepting for players")
        }
    }

    // called once by the game creator to end the lobby
    // pub fn start_game(&mut self, game_id: GameId) {
    //     assert!(self.games.contains_key(&game_id), "game does not exist");

    //     let game = self.games.get(&game_id).unwrap();

    //     match game {
    //         Game::WaitingForPlayers(lobby) => {
    //             let account_id = env::predecessor_account_id();
    //             assert!(lobby.player_account_ids[0] == account_id, "only the creator can start the game");

    //             let GameLobby {
    //                 id: _,
    //                 player_account_ids,
    //                 player_game_pubkeys,
    //             } = lobby;

    //             let state = GameState::new(game_id, player_account_ids, player_game_pubkeys, self.trusted_setup_params.clone());
    //             self.games.insert(&game_id, &Game::InProgress(state));
    //         },
    //         _ => panic!("game is no longer accepting players")
    //     }
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
