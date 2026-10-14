# SOUS-CHEF

sous-chef is a harness for a self-governing, token-efficent agent that you
can work with through Basecamp, HEY & Fizzy like she was your human colege.
The agent is called Anna unless you name it something else (the `name`
setting).

> Jag känner en bot, hon heter Anna, Anna heter hon<br>
> — Basshunter, Botten Anna

She uses your regular Claude Code subscription, runs everything in
sandboxes, can auto-switch between multiple Claude subscriptions,
and learns and improves over time while also being token efficient.

> [!WARNING]
> sous-chef is experimental. This is just an idea I'm playing around with.
> Consider all of this a giant Rube Goldberg machine that's actively
> being tweaked.

## Setting her up

Before you start, you'll have to have Claude Code installed and at least 
one active subscription for it.

The easiest way to install sous-chef is through [mise](https://mise.jdx.dev):

```bash
mise use -g github:monorkin/sous-chef
```

mise hides releases younger than 24 hours by default (`minimum_release_age`).
If the latest release is fresher than that, pin the version instead:

```bash
mise use -g github:monorkin/sous-chef@v0.1.0
```

On [Omarchy](https://omarchy.org):

```bash
omarchy-mise-install github:monorkin/sous-chef sous-chef
```

Her hands run in [bubblewrap](https://github.com/containers/bubblewrap) and
reach their services through `socat`, so install both from your distribution.
If [mr-boxington](https://mr-boxington.jdx.dev) is among your global mise
tools, her hands build Rust through it, with a cache of her own.

Once installed, start the setup process. It will guide you through everything:

```bash
sous-chef setup
```

This project used to be called anna. If you installed it with
`github:monorkin/anna`, reinstall it with `mise use -g github:monorkin/sous-chef`.
Stop the old one first (`anna stop`); the first time sous-chef starts it
moves your folders, Claude sessions, keyring token and service over.

## Upgrading

```bash
sous-chef upgrade           # to the latest release
sous-chef upgrade 0.2.0     # or to a specific one
```

This moves the mise install forward and points her service at the new
binary. She keeps running the old one until you restart her, so pick a
moment when her hands aren't in the middle of something:

```bash
sous-chef restart
```

## Usage

```bash
# Running
sous-chef start                                # bring her up in the background (through systemd, if setup installed the service)
sous-chef stop                                 # stop her and everything she started
sous-chef restart                              # stop and start; she picks up whatever she was in the middle of
sous-chef status
sous-chef poke basecamp                        # check a source now instead of at the next tick; hook this to webhooks, mail filters, cron
sous-chef run                                  # or run her in this terminal until ctrl+c
sous-chef chat "the deploy is red, have a look" # or talk to her from this terminal
sous-chef chat --conversation billing "..."    # each conversation is its own thread
sous-chef log -f                               # watch what she's doing

# Backups
sous-chef backup                               # everything that makes her her, in one zip; safe while she runs
sous-chef backup --to ~/sous-chef.zip --without-secrets
sous-chef restore ~/sous-chef.zip              # she has to be stopped; --force replaces an agent already set up here

# MCP servers: how she acts, and where she listens
sous-chef mcp add basecamp -- basecamp mcp     # also accepts a server's changed tools
sous-chef mcp list
sous-chef mcp prose basecamp create_comment content   # this argument is prose for people: restyle it
sous-chef mcp remove basecamp
sous-chef source check basecamp                # read a source once and show what she'd make of it, without acting

# Subscription management
sous-chef claude account add # add the current Claude Code account to the rotation
sous-chef claude account remove email@example.com
sous-chef claude account list

# Memory
sous-chef memory list
sous-chef memory archive [memory-id]

# Logs
sous-chef log [-f] [--tail]
```

## Architecture

sous-chef is built around the brain and hands principle. In short, this means
that there is a "brain" that accomplishes tasks using its "hands".

The agent's brain consists of a dispatcher and multiple threads, 
each with its own reviewer.

Each hand is a sandboxed instance of Claude Code that can only talk to its brain. It has
very limited disk access, and no internet access besides localhost. All actions it wants
to take, that go beyond the sandbox go through an MPC request that's arbitrated by a
broker.

```
+- SOURCE -----------------------------------------------------------+
|                                                                    |
|            Basecamp / Fizzy / HEY / Github / Sentry                |
|                                                                    |
+----^----------------------------+-----------------------------^----+
     |                            |                             |
     | replies,                   | messages,          replies, |
     | comments                   | assignments        comments |
     |                            |                             |
+----|-- BRAIN -------------------|-----------------------------|----+
|    |                            |                             |    |
|    |               +------------v-------------+               |    |
|    |               |        DISPATCHER        |               |    |
|    |               |  classifies and routes   |               |    |
|    |               +-----+--------------+-----+               |    |
|    |                     |              |                     |    |
|    |             message |              | message             |    |
|    |                     |              |                     |    |
|  +-+------- THREAD ------v----+    +----v---- THREAD ---------+-+  |
|  |  SESSION       REVIEWER    |    |  SESSION       REVIEWER    |  |
|  |  plans, writes checks the  |    |                            |  |
|  |  brief + grant hand's work |    |                            |  |
|  +---+------------^-----------+    +-----+------^------+------^-+  |
|      | brief +    | result +             |      |      |      |    |
|      | grant      | transcript           |      |      |      |    |
|      |            |                      |      |      |      |    |
|  +---v------------+----------------------v------+------v------+-+  |
|  |  BROKER   enforces each hand's grant, carries MCP actions    |  |
|  |           and Claude API traffic                             |  |
|  +---+------------^---------------+------+------^------+------^-+  |
|      |            |               |      |      |      |      |    |
|      |            |  +- MEMORY ---v---+  |      |      |      |    |
|      |            |  | transcript >   |  |      |      |      |    |
|      |            |  | extract > rules|  |      |      |      |    |
|      |            |  | > risk > store |  |      |      |      |    |
|      |            |  +----------------+  |      |      |      |    |
|      |            |                      |      |      |      |    |
+------+------------+----------------------+------+------+------+----+
       |            |                      |      |      |      |
       |            |                      |      |      |      |
   +---v--- HAND ---+-----------+        +-v HAND +-+  +-v HAND +-+
   | Claude Code in a sandbox:  |        |          |  |          |
   | one project folder, no     |        |          |  |          |
   | network, deleted when the  |        |          |  |          |
   | thread is done with it     |        |          |  |          |
   +----------------------------+        +----------+  +----------+
```

## Building

```bash
mise trust && mise install         # rust, mr-boxington, cross, and gh, from mise.toml
cargo build --release              # → target/release/sous-chef
sudo make install                  # → /usr/bin/sous-chef
```

A local `make install` just needs the host toolchain. With mise, cargo runs
through [mr-boxington](https://mr-boxington.jdx.dev), so git worktrees and
sibling checkouts share compiled crates instead of each growing a full
`target/`.

Cutting a release (`make release`, or `make build-all` on its own)
cross-builds static musl binaries for amd64 and arm64, which needs
[`cross`](https://github.com/cross-rs/cross) and a running Docker or Podman:

```bash
make release        # builds both targets, publishes the GitHub release
```

`Cross.toml` pins the `:main` cross images — the default pinned images ship a
glibc too old to run a current rustc's build scripts.

## License

MIT — see [LICENSE](LICENSE).
