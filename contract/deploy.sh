#!/bin/sh

./build.sh

if [ $? -ne 0 ]; then
  echo ">> Error building contract"
  exit 1
fi

echo ">> performing trusted setup"
cargo run --example trusted_setup
PARAMS_JSON=`cat params.json`

echo ">> Deploying contract"
rm -rf neardev
ARGS="{ \"trusted_setup_params\": $PARAMS_JSON }"
near dev-deploy --wasmFile ./target/wasm32-unknown-unknown/release/rainbase_contract.wasm
DEV_ACCOUNT=`cat neardev/dev-account`
near call $DEV_ACCOUNT init "'$ARGS'" --accountId $DEV_ACCOUNT --gas 300000000000000
