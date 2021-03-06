#!/bin/bash

export RUST_BACKTRACE=full
export RUST_LOG=neb=debug

cargo test -- --nocapture --test-threads=1

 fswatch src/ tests/ -e ".*" -i "\\.rs$" | (while read; do cargo test -- --nocapture --test-threads=1; done)
