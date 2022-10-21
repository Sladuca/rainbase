/*
 * Example smart contract written in RUST
 *
 * Learn more about writing NEAR smart contracts with Rust:
 * https://near-docs.io/develop/Contract
 *
 */

use near_sdk::borsh::{self, BorshDeserialize, BorshSerialize};
use near_sdk::log;
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
use poker::{Card, Rank, Suit, Evaluator};

const GAMES_STORAGE_KEY: &'static [u8] = b"GAMES";
const MAPPING_STORAGE_KEY: &'static [u8] = b"CARD_MAPPING";

const LITTLE_BLIND_AMOUNT: Balance = 5;
const BIG_BLIND_AMOUNT: Balance = 10;

type GameId = [u8; 4];

// Define the contract structure
#[near_bindgen]
#[derive(BorshDeserialize, BorshSerialize)]
pub struct Contract {
    games: LookupMap<GameId, Game>,
    trusted_setup_params: BnParamsBuf,
    card_mapping: LookupMap<BnCardBuf, usize>,
}

fn card_value_to_card(mapping: &LookupMap<BnCardBuf, usize>, value: &BnCardBuf) -> Card {
    const SUITS: [Suit; 4] = [Suit::Spades, Suit::Hearts, Suit::Diamonds, Suit::Clubs];
    const RANKS: [Rank; 13] = [
        Rank::Two, Rank::Three, Rank::Four, Rank::Five, Rank::Six, Rank::Seven, Rank::Eight, Rank::Nine, Rank::Ten, Rank::Jack, Rank::Queen, Rank::King, Rank::Ace
    ];

    let card_idx = mapping.get(value).expect("card value not found");
    let rank_idx = card_idx % 13;
    let suit_idx = card_idx / 13;
    Card::new(RANKS[rank_idx], SUITS[suit_idx])
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
    pub revealed_players: Vec<bool>,

    /// the amounts each player has bet so far
    pub bets: Vec<BetAmount>,

    /// the current ante
    pub ante: Balance,

    /// number of players who have checked
    pub checks: Vec<bool>,

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
            ante: 0,
            checks: vec![false; num_players],
            revealed_players: vec![false; num_players],
            bets: vec![BetAmount::In(0); num_players],
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
        self.revealed_players[player_idx] = true;
    }

    fn reset_revealed_players(&mut self) {
        self.revealed_players = vec![false; self.num_players()];
    }

    fn all_players_revealed(&self) -> bool {
        self.revealed_players.iter().all(|&x| x)
    }

    fn num_checks(&self) -> usize {
        self.checks.iter().filter(|&x| *x).count()
    }

    fn reset_checks(&mut self) {
        self.checks = vec![false; self.num_players()];
    }

    fn set_player_checked(&mut self, player_idx: usize) {
        self.checks[player_idx] = true;
    }

    fn unset_player_checked(&mut self, player_idx: usize) {
        self.checks[player_idx] = false;
    }

    fn num_players_in(&self) -> usize {
        (0..self.bets.len()).filter(|&i| !self.player_is_folded(i)).count()
    }

    fn num_players_checked(&self) -> usize {
        self.checks.iter().filter(|&x| *x).count()
    }

    fn enough_players_checked(&self) -> bool {
        self.num_players_in() == self.num_players_checked()
    }

    fn set_folded_player(&mut self, player_idx: usize) {
        self.bets[player_idx] = match self.bets[player_idx] {
            BetAmount::In(amt) => BetAmount::Folded(amt),
            BetAmount::AllIn => BetAmount::Folded(self.balances[player_idx]),
            _ => panic!("player is already folded")
        }
    }

    fn reset_bets(&mut self) {
        self.bets = vec![BetAmount::In(0); self.num_players()];
    }

    fn player_can_check(&self) -> bool {
        let player_idx = self.turn;
        match self.bets[player_idx] {
            BetAmount::In(amt) => amt == self.ante,
            BetAmount::AllIn => true,
            BetAmount::Folded(_) => false
        }
    }

    fn player_can_call(&self) -> bool {
        let player_idx = self.turn;
        match self.bets[player_idx] {
            BetAmount::In(_) => self.balances[player_idx] >= self.ante,
            BetAmount::AllIn => false,
            BetAmount::Folded(_) => false
        }
    }

    fn player_can_all_in(&self) -> bool {
        let player_idx = self.turn;
        match self.bets[player_idx] {
            BetAmount::AllIn => false,
            BetAmount::In(_) => true,
            BetAmount::Folded(_) => false,
        }
    }

    fn player_can_fold(&self) -> bool {
        let player_idx = self.turn;
        match self.bets[player_idx] {
            BetAmount::AllIn => true,
            BetAmount::In(_) => true,
            BetAmount::Folded(_) => false,
        }
    }

    fn player_can_raise(&self) -> bool {
        let player_idx = self.turn;
        match self.bets[player_idx] {
            BetAmount::AllIn => false,
            BetAmount::In(_) => self.balances[player_idx] > self.ante,
            BetAmount::Folded(_) => false,
        }
    }

    fn next_in_player(&self) -> Option<usize> {
        let mut count = 0;
        let mut i = (self.turn + 1) % self.num_players();
        loop {
            if !self.player_is_folded(i) {
                return Some(i);
            }
            i = (i + 1) % self.num_players();
            count += 1;
            if count == self.num_players() {
                return None;
            }
        }
    }

    fn player_is_folded(&self, i: usize) -> bool {
        matches!(self.bets[i], BetAmount::Folded(_))
    }

    fn transfer_pot(&mut self, winner: usize) {
        for (loser, bet) in self.bets.iter().enumerate().filter(|(i, _)| *i != winner) {
            match bet {
                BetAmount::In(amount) => {
                    self.balances[winner] += amount;
                    self.balances[loser] -= amount;
                }
                BetAmount::AllIn => {
                    self.balances[winner] += self.balances[loser];
                    self.balances[loser] = 0;
                }
                BetAmount::Folded(amount) => {
                    self.balances[winner] += amount;
                    self.balances[loser] -= amount;
                }
            }
        }
    }

    fn do_showdown(&mut self, card_mapping: &LookupMap<BnCardBuf, usize>, pp: &BnParameters) {
        let pks = self.player_game_pubkeys.iter().map(|x| x.deserialize().expect("failed to deserialize pubkey")).collect::<Vec<_>>();
        let mut community = Vec::new();
        for i in (self.num_players() * 2..self.num_players() * 2 + 5) {
            let masked_card = self.deck[i].deserialize().expect("failed to deserialize masked community card");
            let reveal_tokens_with_proofs = self.reveal_tokens_with_proofs[i].iter().map(|x| {
                x.as_ref().expect("reveal token not set").deserialize().expect("failed to deserialize reveal token")
            }).collect::<Vec<_>>();
            let decryption_key = reveal_tokens_with_proofs.into_iter().zip(pks.clone()).map(|((token, proof), pk)| (token, proof, pk)).collect();
            let card_value = BnCardProtocol::unmask(pp, &decryption_key, &masked_card, false).expect("failed to unmask card");
            let card_value = BnCardBuf::serialize(card_value).expect("failed to serialize card");
            community.push(card_value_to_card(card_mapping, &card_value));
        }

        let evaluator = Evaluator::new();
        let mut hands = Vec::new();
        for player in (0..self.num_players()).filter(|&i| !self.player_is_folded(i)) {
            let hole_indices = [player * 2, player * 2 + 1];
            let mut hole = Vec::new();

            for i in hole_indices {
                let masked_card = self.deck[i].deserialize().expect("failed to deserialize masked hole card");
                let reveal_tokens_with_proofs = self.reveal_tokens_with_proofs[i].iter().map(|x| {
                    x.as_ref().expect("reveal token not set").deserialize().expect("failed to deserialize reveal token")
                }).collect::<Vec<_>>();
                let decryption_key = reveal_tokens_with_proofs.into_iter().zip(pks.clone()).map(|((token, proof), pk)| (token, proof, pk)).collect();
                let card_value = BnCardProtocol::unmask(pp, &decryption_key, &masked_card, false).expect("failed to unmask card");
                let card_value = BnCardBuf::serialize(card_value).expect("failed to serialize card");
                hole.push(card_value_to_card(card_mapping, &card_value));
            }
            let hand = [hole, community.clone()].concat();
            let hand_eval = evaluator.evaluate(&hand).expect("failed to evaluate hand");
            hands.push((player, hand_eval));
        }

        let &(winner, _) = hands.iter().reduce(|a, b| if a.1 > b.1 { a } else { b }).expect("no winner");
        self.transfer_pot(winner);
    }

    fn new_round(&mut self) {
        self.reset_bets();
        self.reset_checks();
        self.reset_revealed_players();
        self.reset_reveal_tokens();
        self.ante = 0;
        self.turn = self.dealer;
    }

}


#[derive(BorshDeserialize, BorshSerialize)]
pub enum Phase {
    SHUFFLE,
    DEAL,
    BLIND,
    BET0,
    FLOP,
    BET1,
    TURN,
    BET2,
    RIVER,
    BET3,
    SHOWDOWN_REVEAL,
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
        let mut card_mapping = LookupMap::new(MAPPING_STORAGE_KEY);
        for (i, value) in card_values.iter().enumerate() {
            card_mapping.insert(value, &i);
        }
        Self {
            games: LookupMap::new(GAMES_STORAGE_KEY),
            trusted_setup_params: BnParamsBuf { buf: vec![] },
            card_mapping,
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
        let mut card_mapping = LookupMap::new(MAPPING_STORAGE_KEY);
        for (i, value) in card_values.iter().enumerate() {
            card_mapping.insert(value, &i);
        }
        // check serialization of params
        // let _ = trusted_setup_params.deserialize().expect("failed to deserialize trusted setup params");
        Self {
            games: LookupMap::new(GAMES_STORAGE_KEY),
            trusted_setup_params,
            card_mapping
        }
    }

    pub fn get_params(&self) -> BnParamsBuf {
        self.trusted_setup_params.clone()
    }

    pub fn get_aggregate_pubkey(&self, game_id: GameId) -> BnPublicKeyBuf {
        let game = self.games.get(&game_id).expect("game not found");
        let game = match game {
            Game::WaitingForPlayers(_) => panic!("game not in progress"),
            Game::InProgress(game) => game,
        };

        game.aggregate_pubkey.clone()
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
        let creator_account_id_bytes = creator_account_id.as_bytes().to_vec();

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

                let account_id_bytes = account_id.as_bytes().to_vec();
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

                state.new_round();
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
                    state.phase = Phase::BLIND;
                    state.turn = (state.dealer + 1) % state.num_players();
                }
            },
            _ => panic!("game is not in progress")
        }
    }

    // blind
    pub fn blind(&mut self, game_id: GameId) {
        assert!(self.games.contains_key(&game_id), "game does not exist");

        let mut game = self.games.get(&game_id).unwrap();
        match game {
            Game::InProgress(ref mut state) => {
                assert!(matches!(state.phase, Phase::BLIND), "game is not in the blind phase");
                let account_id = env::predecessor_account_id();
                assert!(state.player_account_ids.contains(&account_id), "only players can blind");

                let player_index = state.player_account_ids.iter().position(|id| id == &account_id).unwrap();
                assert!(state.turn == player_index, "it is not your turn to blind");

                let player_balance = state.balances[player_index];
                let blind_amount = if state.turn == (state.dealer + 1) % state.num_players() { LITTLE_BLIND_AMOUNT } else { BIG_BLIND_AMOUNT };
                if player_balance < blind_amount {
                    state.bets[player_index] = BetAmount::AllIn;
                    state.ante = state.balances[player_index];
                } else {
                    state.bets[player_index] = BetAmount::In(blind_amount);
                    state.ante = blind_amount;
                }

                if state.turn == (state.dealer + 3) % state.num_players() {
                    state.phase = Phase::BET0;
                }
            },
            _ => panic!("game is not in progress")
        } 
    }


    // place bet - players call this in turn order until the betting is done. this is only called during the bet phases
    pub fn bet(&mut self, game_id: GameId, call: bool, check: bool, all_in: bool, fold: bool, raise: Option<Balance>) {
        assert!(self.games.contains_key(&game_id), "game does not exist");

        let mut game = self.games.get(&game_id).unwrap();
        match game {
            Game::InProgress(ref mut state) => {
                assert!(matches!(state.phase, Phase::BET0 | Phase::BET1 | Phase::BET2 | Phase::BET3), "game is not in a bet phase");

                let account_id = env::predecessor_account_id();
                assert!(state.player_account_ids.contains(&account_id), "only players can bet");
                let player = state.player_account_ids.iter().position(|id| id == &account_id).unwrap();
                assert!(state.turn == player, "it is not your turn to bet");

                match (call, check, all_in, fold, raise) {
                    // call
                    (true, false, false, false, None) => {
                        assert!(state.player_can_call(), "you cannot call");
                        state.bets[player] = BetAmount::In(state.ante);
                        state.reset_checks();
                    }
                    // check
                    (false, true, false, false, None) => {
                        assert!(state.player_can_check(), "you cannot check");
                        state.set_player_checked(player);
                    }
                    // all in
                    (false, false, true, false, None) => {
                        assert!(state.player_can_all_in(), "you cannot all in");
                        state.bets[player] = BetAmount::AllIn;
                        if state.balances[player] > state.ante {
                            state.ante = state.balances[player];
                        }
                        state.reset_checks()
                    }
                    // fold
                    (false, false, false, true, None) => {
                        assert!(state.player_can_fold(), "you cannot fold");
                        state.bets[player] = match state.bets[player] {
                            BetAmount::In(amount) => BetAmount::Folded(amount),
                            BetAmount::AllIn => BetAmount::Folded(state.balances[player]),
                            _ => unreachable!()
                        };
                        state.checks[player] = false;
                    }
                    // raise
                    (false, false, false, false, Some(raise_amount)) => {
                        assert!(state.player_can_raise(), "you cannot raise");
                        assert!(raise_amount > state.ante, "raise amount must be greater than the ante");
                        assert!(raise_amount <= state.balances[player], "raise amount must be less than or equal to your balance");
                        state.bets[player] = BetAmount::In(raise_amount); 
                        state.ante = raise_amount;
                        state.reset_checks();
                    }
                    _ => panic!("invalid bet flags")
                };

                if state.num_players_in() == 1 {
                    // that player won  
                    let winner = state.next_in_player().unwrap();
                    state.transfer_pot(winner);
                    state.phase = Phase::SHUFFLE;
                    state.reset_bets();
                    state.reset_checks();
                    state.reset_revealed_players();
                } else if state.enough_players_checked() {
                    // move to next phase
                    state.phase = match state.phase {
                        Phase::BET0 => Phase::FLOP,
                        Phase::BET1 => Phase::TURN,
                        Phase::BET2 => Phase::RIVER,
                        Phase::BET3 => Phase::SHOWDOWN_REVEAL,
                        _ => unreachable!()
                    };
                    state.reset_checks();
                } else {
                    // move to next player
                    state.turn = state.next_in_player().unwrap();
                }

                state.turn = state.next_in_player().expect("next player should exist");
            },
            _ => panic!("game is not in progress")
        }
    }

    // reveal cards - each player has to call this (any order) with their reveal tokens calculated client side. number of cards revealed depends on the phase
    pub fn reveal(&mut self, game_id: GameId, card_indices: Vec<usize>, reveal_tokens_with_proofs: Vec<BnRevealTokenWithProofBuf>) {
        assert!(self.games.contains_key(&game_id), "game does not exist");

        let mut game = self.games.get(&game_id).unwrap();
        match game {
            Game::InProgress(ref mut state) => {
                let account_id = env::predecessor_account_id();
                assert!(state.player_account_ids.contains(&account_id), "only players can reveal");
                let player = state.player_account_ids.iter().position(|id| id == &account_id).unwrap();
                assert!(!state.revealed_players[player], "you have already revealed");

                let _indices_should_reveal: Vec<usize> = match state.phase {
                    Phase::FLOP => {
                        ((state.num_players() * 2)..(state.num_players() * 2 + 3)).collect()
                    },
                    Phase::TURN => {
                        ((state.num_players() * 2 + 3)..(state.num_players() * 2 + 4)).collect()
                    },
                    Phase::RIVER => {
                        ((state.num_players() * 2 + 4)..(state.num_players() * 2 + 5)).collect()
                    },
                    Phase::SHOWDOWN_REVEAL => {
                       vec![player * 2, player * 2 + 1]
                    }
                    _ => panic!("cannot reveal cards in this phase")
                };

                // TODO check indices are sorted, unique, same len as reveal tokens, and match indices_should_reveal
                let num_cards = match state.phase {
                    Phase::FLOP => 3,
                    Phase::TURN => 1,
                    Phase::RIVER => 1,
                    Phase::SHOWDOWN_REVEAL => 2,
                    _ => unreachable!()
                };

                assert!(card_indices.len() == num_cards, "wrong number of cards revealed");
                assert!(reveal_tokens_with_proofs.len() == num_cards, "wrong number of reveal tokens revealed");

                let pp = self.trusted_setup_params.deserialize().expect("failed to deserialize trusted setup params");
                let pk = state.player_game_pubkeys[player].deserialize().expect("failed to deserialize player pubkey");

                for (card_idx, reveal_token_with_proof) in card_indices.into_iter().zip(reveal_tokens_with_proofs) {
                    let masked_card = state.deck[card_idx].deserialize().expect("failed to deserialize masked card");
                    let (reveal_token, proof) = reveal_token_with_proof.deserialize().expect("failed to deserialize reveal token with proof");
                    BnCardProtocol::verify_reveal(&pp, &pk, &reveal_token, &masked_card, &proof).expect("failed to verify reveal token proof");
                    state.set_reveal_token(card_idx, player, reveal_token_with_proof);
                }

                state.revealed_players[player] = true;

                if state.all_players_revealed() {
                    state.phase = match state.phase {
                        Phase::FLOP => Phase::BET1,
                        Phase::TURN => Phase::BET2,
                        Phase::RIVER => Phase::BET3,
                        Phase::SHOWDOWN_REVEAL => Phase::SHOWDOWN,
                        _ => unreachable!()
                    };

                    if let Phase::SHOWDOWN = state.phase {
                        state.do_showdown(&self.card_mapping, &pp);
                        state.phase = Phase::SHUFFLE;
                        state.dealer = (state.dealer + 1) % state.num_players();
                        state.new_round();
                    } else {
                        state.turn = state.dealer;
                        if state.player_is_folded(state.dealer) {
                            state.turn = state.next_in_player().expect("next player should exist");
                        }
                        state.reset_revealed_players();
                    }
                }
            },
            _ => panic!("game is not in progress")
        }
    }
}

/*
 * The rest of this file holds the inline tests for the code above
 * Learn more about Rust tests: https://doc.rust-lang.org/book/ch11-01-writing-tests.html
 */
#[cfg(test)]
mod tests {
    // TODO
}

