#!/bin/bash -e
rm -f ../wasmer_term/public/bin/deploy.wasm

cargo wasix build --features client_web,force_tty --no-default-features

cp -f ../target/wasm32-wasmer-wasi/debug/wasmer-deploy.wasm ../wasmer-web/public/bin/deploy.wasm
chmod +x ../wasmer-web/public/bin/deploy.wasm
