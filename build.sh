#!/bin/bash
cd /Users/charles/Projects/claudia
cargo build --release
cp target/release/claudia ~/.local/bin/claudia
echo "Build complete!"