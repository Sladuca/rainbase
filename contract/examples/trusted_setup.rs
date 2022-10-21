use rand::{Rng, thread_rng};
use barnett_smart_card_protocol::{
	BarnettSmartProtocol,
	discrete_log_cards::{
		BnParameters,
		BnParamsBuf,
		BnCardProtocol
	}
};
use std::fs::File;
use std::io::Write;

const M: usize = 2;
const N: usize = 26;

fn main() {
	let mut rng = thread_rng();
	let params = BnCardProtocol::setup(&mut rng, M, N).unwrap();
	let params_buf = BnParamsBuf::serialize(params).unwrap();
	let params_json = serde_json::to_vec(&params_buf).unwrap();

	let mut file = File::create("params.json").unwrap();
	file.write_all(&params_json).unwrap();
}