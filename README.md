# albo

A curated directory of skilled people, built from their Instagram handles.

*albo* is the Italian word for an official register of a profession (an
*albo* of lawyers, of architects). This is that, for whatever trade you
point it at: tattooers, barbers, ceramicists, photographers. You add
people by their Instagram handle, tag them by style, and get a fast,
browsable, filterable public directory that links back to their work.

One binary, one directory. Everything that makes an instance *this*
directory (its name, what an entry is called, the tag taxonomy) lives in a
config file; the engine is generic.

## What it does

- **Add by handle.** Paste an Instagram handle or profile URL; albo does a
  best-effort fetch of the public profile's name and avatar. No login, no
  API keys. If the fetch fails, you just type the fields.
- **Curate the portfolio.** Paste a few of an artist's public post URLs;
  they render on the artist page as official Instagram embeds. albo never
  scrapes posts - you (or the artist) choose what represents them.
- **Browse and filter.** The public directory lists active entries and
  filters by style tag.
- **Simple admin.** A login-gated form to add, edit, and remove entries.
  Admin accounts live in the database, not in config.

## Why not just scrape Instagram?

Because you can't, sustainably. Instagram serves public profile *metadata*
(name, avatar) to a plain request, but everything else - the post grid, an
artist's recent work - is behind authenticated JavaScript, and the routes
that used to expose it now return login walls. Scraping with a fake account
gets that account flagged and violates the terms of service. So albo takes
only what's freely and legitimately available (profile og-tags, official
post embeds you supply) and treats Instagram as a source to *reference*,
not mirror.

## Quick start

```bash
# Build
cargo build --release

# Copy and edit the config for your directory
cp directory.example.toml directory.toml
$EDITOR directory.toml

# Create the first admin (prompts for a password; or pipe one in)
./target/release/albo --config directory.toml admin-add yourname

# Run
./target/release/albo --config directory.toml serve
# -> serving on the bind address from directory.toml (default 127.0.0.1:3010)
```

Then visit the site, log in at `/admin`, and add people.

## Configuration

`directory.toml` (see `directory.example.toml`):

```toml
[directory]
name = "Meat Ledger"              # the instance's brand
entity = "tattooer"               # singular label for one entry
entities = "tattooers"            # plural
tagline = "Portland tattooers, curated"

[tags]
available = ["traditional", "fine line", "blackwork", "color", "flash"]

[server]
bind = "127.0.0.1:3010"
database = "albo.db"
```

No secrets live here. Admin accounts are in the database, managed by CLI:

```bash
albo --config directory.toml admin-add <username>     # create / reset password
albo --config directory.toml admin-list
albo --config directory.toml admin-remove <username>
```

## Data & files

An instance keeps its state in its working directory: the SQLite database
(`database` in config) and an `avatars/` folder of cached profile images.
Both are runtime data - back them up, don't commit them. Put a reverse
proxy (Caddy, nginx) in front for TLS; albo binds localhost by design.

## Deploying

albo is a single self-contained binary (templates are compiled in).
Tagged releases (`v*`) build a Linux x86_64 binary via GitHub Actions.
For a NixOS host, fetch the pinned release binary and run it as a systemd
service behind your reverse proxy, with a writable state directory for the
database and avatars.

## License

MIT
