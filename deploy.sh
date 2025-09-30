#!/bin/bash

# Deployment script for noreposts-atproto-feed
# Deploys Rust feed generator to Hetzner server
#
# NOTE: Nginx configuration is handled separately in the vitorpy.com repo

set -e  # Exit on any error

# Configuration
SERVER="root@167.235.24.234"
REMOTE_DIR="/var/www/noreposts-feed"
SERVICE_NAME="noreposts-feed"

echo "🚀 Starting deployment to noreposts-feed..."

# Step 1: Build the Rust binary
echo "📦 Building Rust binary..."
cargo build --release

if [ $? -ne 0 ]; then
    echo "❌ Cargo build failed!"
    exit 1
fi

echo "✅ Rust build complete"

# Step 2: Upload binary and SQL migration
echo "📤 Uploading binary and files..."
ssh $SERVER "mkdir -p $REMOTE_DIR"
scp target/release/following-no-reposts-feed $SERVER:$REMOTE_DIR/
scp 001_initial.sql $SERVER:$REMOTE_DIR/

if [ $? -ne 0 ]; then
    echo "❌ Upload failed!"
    exit 1
fi

echo "✅ Files uploaded"

# Step 3: Set correct permissions
echo "🔒 Setting permissions..."
ssh $SERVER "chmod +x $REMOTE_DIR/following-no-reposts-feed"

# Step 4: Restart the service
echo "🔄 Restarting service..."
ssh $SERVER "systemctl restart $SERVICE_NAME"
ssh $SERVER "systemctl status $SERVICE_NAME --no-pager"

echo "✅ Deployment complete!"
echo "🌐 Feed is live"