/*
 * Example smart contract written in RUST
 *
 * Learn more about writing NEAR smart contracts with Rust:
 * https://near-docs.io/develop/Contract
 *
 */

use near_sdk::borsh::{self, BorshDeserialize, BorshSerialize};
use near_sdk::{
    near_bindgen,
    AccountId,
    Balance, 
    env,
    collections::{
        LookupMap,
    }
};
use is_sorted::IsSorted;
use barnett_smart_card_protocol::{
    BarnettSmartProtocol,
    discrete_log_cards::{
        BnPublicKeyBuf,
        BnParamsBuf,
        BnMaskedCardBuf,
        BnRevealTokenWithProofBuf,
        BnZKProofKeyOwnershipBuf,
        BnShuffleOutputBuf,
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

    /// current phase of the game
    pub phase: Phase,

    /// the current player whose turn it is
    pub turn: usize,

    /// the current "dealer". Since everyone shuffles, in practice this is just the player who publishes the initial deck for the round
    /// this is also used to determine who the little blind, big blind, and action is
    /// this increases by one each round
    pub dealer: usize,

    /// used to check which players have revealed.
    pub revealed_players: Vec<Option<()>>,

    /// the amounts each player has bet so far
    pub bets: Vec<BetAmount>,

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

#[derive(BorshDeserialize, BorshSerialize, Clone, Debug)]
pub enum BetAmount {
    /// the player is all-in
    AllIn,
    
    /// the player bet some amount, isn't all-in, and hasn't folded
    In(Balance),

    /// the player folded and is out of the round with their ante still on the table
    Folded(Balance)
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
            phase: Phase::SHUFFLE,
            turn: 0,
            dealer: 0,
            revealed_players: vec![None; num_players],
            bets: vec![BetAmount::In(0); num_players],
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

    fn set_deck(&mut self, deck: Vec<BnMaskedCardBuf>) {
        self.deck = deck;
    }

    fn reset_reveal_tokens(&mut self) {
        self.reveal_tokens_with_proofs = vec![vec![None; self.num_players()]; self.num_cards()];
    }

    fn set_reveal_token(&mut self, card_idx: usize, player_idx: usize, token: BnRevealTokenWithProofBuf) {
        self.reveal_tokens_with_proofs[card_idx][player_idx] = Some(token);
    }

    fn set_revealed_player(&mut self, player_idx: usize) {
        self.revealed_players[player_idx] = Some(());
    }

    fn reset_revealed_players(&mut self) {
        self.revealed_players = vec![None; self.num_players()];
    }

    fn all_players_revealed(&self) -> bool {
        self.revealed_players.iter().all(|x| x.is_some())
    }
}


#[derive(BorshDeserialize, BorshSerialize)]
pub enum Phase {
    SHUFFLE,
    DEAL,
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

// this should not be used. for now it's just gonna put an empty buffer. eventually this will panic.
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
    #[init]
    #[private]
    pub fn init(trusted_setup_params: BnParamsBuf) -> Self {
        let card_values = get_card_elems_buf(52).unwrap();
        // check serialization of params
        // let _ = trusted_setup_params.deserialize().expect("failed to deserialize trusted setup params");
        Self {
            games: LookupMap::new(GAMES_STORAGE_KEY),
            trusted_setup_params,
            card_values,
        }
    }

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
    pub fn start_game(&mut self, game_id: GameId) {
        assert!(self.games.contains_key(&game_id), "game does not exist");

        let game = self.games.get(&game_id).unwrap();

        match game {
            Game::WaitingForPlayers(lobby) => {
                let account_id = env::predecessor_account_id();
                assert!(lobby.player_account_ids[0] == account_id, "only the creator can start the game");

                let GameLobby {
                    id: _,
                    player_account_ids,
                    player_game_pubkeys,
                    created_at: _,
                } = lobby;

                let state = GameState::new(game_id, player_account_ids, player_game_pubkeys, self.trusted_setup_params.clone());
                self.games.insert(&game_id, &Game::InProgress(state));
            },
            _ => panic!("game is no longer accepting players")
        }
    }

    // init the deck - game creator calls this once at the beginning
    // TODO (later): verify the masking proofs. There's a lot of them and it's probably a pain so I'm skipping it for now (oopsies)
    pub fn init_deck(&mut self, game_id: GameId, deck: Vec<BnMaskedCardBuf>) {
        assert!(self.games.contains_key(&game_id), "game does not exist");

        let mut game = self.games.get(&game_id).unwrap();
        match game {
            Game::InProgress(ref mut state) => {
                let account_id = env::predecessor_account_id();
                assert!(state.player_account_ids[0] == account_id, "only the creator can init the deck");
                assert!(state.deck.len() == 0, "deck must not have been initialized yet");
                assert!(deck.len() == 52, "deck must have 52 cards");

                state.reset_reveal_tokens();
                state.set_deck(deck)
            },
            _ => panic!("game is not in progress")
        }
    }

    // shuffle the deck - each player calls this going around one at a time in turn order - the dealer calls this first
    pub fn shuffle_deck(&mut self, game_id: GameId, shuffle: BnShuffleOutputBuf) {
        assert!(self.games.contains_key(&game_id), "game does not exist");

        let mut game = self.games.get(&game_id).unwrap();
        match game {
            Game::InProgress(ref mut state) => {
                let account_id = env::predecessor_account_id();
                assert!(state.player_account_ids.contains(&account_id), "only players can shuffle the deck");

                let player_index = state.player_account_ids.iter().position(|id| id == &account_id).unwrap();
                assert!(state.turn == player_index, "it is not your turn to shuffle the deck");

                let (deck, proof) = shuffle.deserialize().expect("failed to deserialize shuffle");
                let pp = self.trusted_setup_params.deserialize().expect("failed to deserialize trusted setup params");
                let aggregate_pubkey = state.aggregate_pubkey.deserialize().expect("failed to deserialize aggregate pubkey");
                let mut old_deck = Vec::new();
                for card in state.deck.iter() {
                    old_deck.push(card.deserialize().expect("failed to deserialize card"));
                }

                BnCardProtocol::verify_shuffle(&pp, &aggregate_pubkey, &old_deck, &deck, &proof).expect("failed to verify shuffle proof");

                let mut shuffled_deck = Vec::new();
                for card in deck {
                    let card = BnMaskedCardBuf::serialize(card).expect("failed to serialize masked card");
                    shuffled_deck.push(card);
                }
                state.set_deck(shuffled_deck);
                state.reset_reveal_tokens();
                state.turn = (state.turn + 1) % state.num_players();

                if state.turn == state.dealer {
                    state.phase = Phase::DEAL;
                }
            },
            _ => panic!("game is not in progress")
        }
    }

    // deal everyone their two cards - each player has to call (any order) this with their reveal tokens calculated client-side.
    pub fn deal(&mut self, game_id: GameId, card_indices: Vec<usize>, reveal_tokens_with_proofs: Vec<BnRevealTokenWithProofBuf>) {
        assert!(self.games.contains_key(&game_id), "game does not exist");

        let mut game = self.games.get(&game_id).unwrap();
        match game {
            Game::InProgress(ref mut state) => {
                assert!(matches!(state.phase, Phase::DEAL), "game is not in the deal phase");
                let account_id = env::predecessor_account_id();
                assert!(state.player_account_ids.contains(&account_id), "only players can deal");

                let player_index = state.player_account_ids.iter().position(|id| id == &account_id).unwrap();

                // player at idx i gets revealed 2*i, 2*i+1
                // => player at idx i should reveal every card but those cards
                // TODO checks later
                // assert!(card_indices.len() == reveal_tokens_with_proofs.len(), "card indices and reveal tokens with proofs should have same len");
                // assert!(card_indices.iter().is_sorted(), "card indices must be sorted");
                // let expected_card_indices = (0..2*state.num_players()).filter(|&i| player_index * 2 != i && player_index * 2 + 1 != i).collect::<Vec<usize>>();
                // assert!(card_indices.len() == expected_card_indices.len(), "you must reveal the correct number of cards");
                // assert!(card_indices.iter().all(|i| expected_card_indices.contains(i)), "you must reveal the correct cards");
                // let prededup_len = card_indices.len();
                // card_indices.dedup();
                // assert!(card_indices.len() == prededup_len, "card indices cannot have duplicates");

                let pp = self.trusted_setup_params.deserialize().expect("failed to deserialize trusted setup params");
                let pk = state.player_game_pubkeys[player_index].deserialize().expect("failed to deserialize player pubkey");

                for (card_idx, reveal_token_with_proof) in card_indices.into_iter().zip(reveal_tokens_with_proofs) {
                    let (reveal_token, proof) = reveal_token_with_proof.deserialize().expect("failed to deserialize reveal token with proof");
                    let masked_card = state.deck[card_idx].deserialize().expect("failed to deserialize masked card");
                    BnCardProtocol::verify_reveal(&pp, &pk, &reveal_token, &masked_card, &proof).expect("failed to verify reveal token proof");
                    state.set_reveal_token(card_idx, player_index, reveal_token_with_proof);
                }

                state.set_revealed_player(player_index);

                if state.all_players_revealed() {
                    state.phase = Phase::BET0;
                }
            },
            _ => panic!("game is not in progress")
        }
    }


    // place bet - players call this in turn order until the betting is done. this is only called during the bet phases

    // reveal cards - each player has to call this (any order) with their reveal tokens calculated client side. number of cards revealed depends on the phase


}

/*
 * The rest of this file holds the inline tests for the code above
 * Learn more about Rust tests: https://doc.rust-lang.org/book/ch11-01-writing-tests.html
 */
#[cfg(test)]
mod tests {
    // TODO
}

