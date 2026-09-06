social media aggregator

multiple social media networks and accounts within those networks

aggregate across accounts on the same topic

TUI interface

Rust language

Facebook, Reddit, X/Twitter, Instagram (including threads), Tiktok

initially limited to text and image posts


### Included

 - Multiple accounts per network:
   - Facebook
   - Reddit
   - X/Twitter
   - Instagram
   - Threads
   - TikTok
 - Unified normalized post model for text and image posts
 - Topic-based aggregation across accounts and networks
 - Stable chronological ordering with deterministic tie-breaking
 - Pagination and deduplication
 - Per-account sync cursors
 - Retry handling and network-specific rate limits
 - Persistent SQLite storage
 - Secure credential indirection:
   - environment variables
   - command-based secret providers
 - Ratatui interface with:
   - account management
   - topic management
   - filtered feed
   - post details
   - sync progress
   - image preview integration
 - Runtime image protocols:
   - Kitty
   - iTerm2
   - Sixel
   - Unicode fallback
 - Full-screen image viewer
 - OSC 8 clickable links
 - Config migration support
 - Embedded SQLite migrations
 - CLI commands for account/topic management and synchronization
 - Updated README and sample configuration

### Verification

 - cargo fmt --check
 - cargo test --all-targets
 - cargo clippy --all-targets --all-features -- -D warnings
 - Temporary SQLite smoke test:
   - configuration initialization
   - account add/list
   - topic add/list
   - sync execution
 - Interactive TUI smoke test in a real PTY:
   - launch
   - accounts screen
   - topics screen
   - feed screen
   - clean quit