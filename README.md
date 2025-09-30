# Following No Reposts Feed Generator

A Rust-based Bluesky feed generator that shows posts from people you follow, excluding all reposts. Built using Jetstream for efficient real-time data consumption.

## Features

- **Efficient Jetstream Integration**: Uses Bluesky's Jetstream service for lightweight, filtered event consumption
- **No Reposts**: Automatically filters out all reposts, showing only original posts
- **Personalized**: Shows only posts from accounts you follow
- **Real-time**: Updates in real-time as new posts and follows are created
- **Memory Efficient**: Automatic cleanup of old posts (configurable retention period)
- **Production Ready**: Includes proper JWT authentication, error handling, and logging

## Architecture

The feed generator consists of several components:

1. **Jetstream Consumer**: Connects to Bluesky's Jetstream and consumes `app.bsky.feed.post` and `app.bsky.graph.follow` events
2. **Database Layer**: SQLite database for storing posts and follow relationships
3. **Web Server**: Axum-based HTTP server that implements the feed skeleton API
4. **Feed Algorithm**: Generates personalized feeds based on user follows
5. **Authentication**: JWT validation for personalized feeds

## Setup

### 1. Prerequisites

Ensure you have Rust installed:

```fish
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

### 2. Clone and Build

```fish
git clone <your-repo>
cd following-no-reposts-feed
cargo build --release
```

### 3. Configuration

Copy the example environment file and configure it:

```fish
cp .env.example .env
nvim .env
```

Required environment variables:
- `FEEDGEN_HOSTNAME`: Your domain where the feed will be hosted
- `FEEDGEN_SERVICE_DID`: Your service DID (usually `did:web:your-domain.com`)
- `DATABASE_URL`: SQLite database path
- `PORT`: Server port (default: 3000)

### 4. Database Setup

The database will be automatically migrated on startup, but you can run migrations manually:

```fish
cargo install sqlx-cli
sqlx migrate run
```

### 5. Run the Feed Generator

```fish
cargo run --release
```

Or with custom parameters:

```fish
cargo run --release -- --port 3000 --hostname your-domain.com
```

## Deployment

### 1. Build for Production

```fish
cargo build --release
```

### 2. Deploy to Your Server

The binary needs to be accessible via HTTPS on port 443. You can use any reverse proxy (nginx, caddy, etc.) to handle TLS termination.

Example nginx configuration:

```nginx
server {
    listen 443 ssl;
    server_name your-domain.com;
    
    ssl_certificate /path/to/cert.pem;
    ssl_certificate_key /path/to/key.pem;
    
    location / {
        proxy_pass http://localhost:3000;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
    }
}
```

### 3. Publishing the Feed

Use the Bluesky API to publish your feed generator. You'll need to create a feed generator record in your account:

```fish
# Get your account DID
curl "https://bsky.social/xrpc/com.atproto.identity.resolveHandle?handle=yourhandle.bsky.social"

# Publish the feed (you'll need to implement this using atrium-api or similar)
```

## API Endpoints

### GET /.well-known/did.json

Returns the DID document for your feed generator service.

### GET /xrpc/app.bsky.feed.getFeedSkeleton

Main feed endpoint that returns the skeleton of posts for the requesting user.

Query parameters:
- `feed`: The AT-URI of the feed (required)
- `limit`: Number of posts to return (optional, max 100)
- `cursor`: Pagination cursor (optional)

Headers:
- `Authorization`: Bearer JWT token for user authentication

## Performance

### Data Usage

Using Jetstream significantly reduces bandwidth compared to the raw firehose:
- **Jetstream**: ~850 MB/day for all posts (with compression)
- **Raw Firehose**: 200+ GB/day during high activity periods

### Filtering Efficiency

The feed generator only subscribes to relevant collections:
- `app.bsky.feed.post` - for post creation/deletion
- `app.bsky.graph.follow` - for follow relationships

Reposts (`app.bsky.feed.repost`) are automatically excluded by not subscribing to that collection.

### Database Optimization

- Automatic cleanup of posts older than 48 hours
- Efficient indexes on author_did and indexed_at
- Unique constraints on follow relationships

## Monitoring

The application includes structured logging. Set `RUST_LOG=debug` for detailed logs.

Key metrics to monitor:
- Database size growth
- Jetstream connection health
- Feed generation latency
- Error rates

## Development

### Running Tests

```fish
cargo test
```

### Database Migrations

Add new migrations in the `migrations/` directory:

```fish
sqlx migrate add your_migration_name
```

### Adding New Features

The modular design makes it easy to extend:

1. **New Event Types**: Add handlers in `jetstream_consumer.rs`
2. **New Algorithms**: Implement new feed algorithms in `feed_algorithm.rs`
3. **Enhanced Auth**: Improve JWT validation in `auth.rs`

## Troubleshooting

### Common Issues

1. **Jetstream Connection Failures**
   - Check network connectivity
   - Verify Jetstream hostname in configuration
   - Monitor for rate limiting

2. **Database Locks**
   - Ensure proper connection pooling
   - Check for long-running transactions

3. **Authentication Errors**
   - Verify JWT implementation matches AT Protocol specs
   - Check DID resolution for user verification keys

### Debugging

Enable debug logging:

```fish
RUST_LOG=debug cargo run
```

Test the feed endpoint directly:

```fish
curl "http://localhost:3000/xrpc/app.bsky.feed.getFeedSkeleton?feed=at://your-did/app.bsky.feed.generator/following-no-reposts&limit=10"
```

## License

MIT License - see LICENSE file for details.

## Contributing

1. Fork the repository
2. Create a feature branch
3. Make your changes
4. Add tests if applicable
5. Submit a pull request

## Resources

- [AT Protocol Documentation](https://atproto.com)
- [Bluesky API Reference](https://docs.bsky.app)
- [Jetstream Documentation](https://github.com/bluesky-social/jetstream)
- [ATrium Rust Library](https://github.com/sugyan/atrium)
