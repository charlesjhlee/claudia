#!/bin/bash

# Claudia Installation Script

set -e

echo "🔨 Building Claudia..."

# Build the project in release mode
cargo build --release

# Note: Not stripping the binary on macOS as it can cause issues with ARM64 binaries
# strip target/release/claudia

# Get the binary size
SIZE=$(du -h target/release/claudia | cut -f1)
echo "✅ Build complete! Binary size: $SIZE"

# Ask user for installation location
echo ""
echo "Where would you like to install claudia?"
echo "1) System-wide (/usr/local/bin) - requires sudo"
echo "2) User only (~/.local/bin)"
echo "3) Custom location"
echo "4) Don't install, just build"

read -p "Choose [1-4]: " choice

case $choice in
    1)
        echo "Installing to /usr/local/bin (requires sudo)..."
        sudo cp target/release/claudia /usr/local/bin/
        echo "✅ Installed to /usr/local/bin/claudia"
        ;;
    2)
        mkdir -p ~/.local/bin
        cp target/release/claudia ~/.local/bin/
        echo "✅ Installed to ~/.local/bin/claudia"
        echo ""
        echo "⚠️  Make sure ~/.local/bin is in your PATH:"
        echo '    export PATH="$HOME/.local/bin:$PATH"'
        ;;
    3)
        read -p "Enter custom installation path: " custom_path
        cp target/release/claudia "$custom_path"
        echo "✅ Installed to $custom_path"
        ;;
    4)
        echo "✅ Binary available at: target/release/claudia"
        ;;
    *)
        echo "Invalid choice. Binary available at: target/release/claudia"
        exit 1
        ;;
esac

echo ""
echo "🎉 Installation complete!"
echo ""
echo "Usage: claudia <markdown_file>"
echo "Example: claudia tasks.md"