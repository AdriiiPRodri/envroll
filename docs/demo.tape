# envroll — landing-page demo (~75 seconds)
#
# Regenerate with:  vhs docs/demo.tape  (from the repo root)
#
# Requires the local release binary built with demo-mode support:
#   cargo build --release
#
# The setup block under `Hide` does four things:
#   1. Puts ./target/release on PATH so this tape uses the local build.
#   2. Wipes the developer's real envroll vault so the demo starts clean
#      and `envroll init` shows the canonical default path
#      (/Users/<you>/.local/share/envroll) instead of an ugly tempdir.
#   3. Drops the demo into a fresh tempdir with a fake git remote so the
#      project ID derives nicely as `myapp`. The project root path itself
#      is never printed during the demo, so the /var/folders prefix
#      doesn't matter.
#   4. Customizes PS1 to include a leading blank line on every prompt.
#      This gives every command vertical breathing room WITHOUT needing
#      to type a no-op `echo` between them — much cleaner than wrapping
#      `echo` calls in Hide/Show, which still leaves the typed `echo`
#      visible in the post-Show captured frame.

Output docs/demo.gif
Output docs/demo.mp4

# ---- Visual setup ----
Set Theme "Catppuccin Mocha"
Set FontFamily "Menlo"
Set FontSize 18
Set Width 1200
Set Height 720
Set Padding 30
Set Margin 0
Set TypingSpeed 50ms

# ---- Hidden setup ----
Hide
Type 'export PATH=/Users/adriiiprodri/git/envroll/target/release:$PATH'
Enter
Type 'rm -rf ~/.local/share/envroll'
Enter
Type 'cd "$(mktemp -d)"'
Enter
Type 'git init -q && git remote add origin git@github.com:acme/myapp.git'
Enter
Type 'export ENVROLL_DEMO_MODE=1 ENVROLL_PASSPHRASE=demo'
Enter
Type 'clear'
Enter
Show

# ---- Act 1: a typical .env file ----
Type 'echo "DATABASE_URL=postgres://localhost/dev" > .env'
Enter
Sleep 700ms
Type 'echo "DEBUG=true" >> .env'
Enter
Sleep 700ms
Type 'cat .env'
Enter
Sleep 1800ms

# Bare Enter (no Type before it) → empty command line in bash → an extra
# blank prompt appears as a visual separator. Not a true blank line, but
# zero typed-command artifact in the recording.
Enter
Sleep 100ms

# ---- Act 2: init + adopt the .env ----
# Sleeps for crypto-heavy commands are sized for: ~600ms passphrase
# animation + ~700ms scrypt verify + ~700-1500ms encrypt/commit + ~2s
# reading. Undersizing causes the next Type to start while the previous
# command is still printing its output, producing visible char interleaving.
Type 'envroll init'
Enter
Sleep 7s

Enter
Sleep 100ms

Type 'envroll fork dev'
Enter
Sleep 6s

Enter
Sleep 100ms

# Show that ./.env is now a symlink into the encrypted vault
Type 'ls -la .env'
Enter
Sleep 3s

Enter
Sleep 100ms

# ---- Act 3: branch, edit, save ----
Type 'envroll fork staging -m "snapshot before db migration"'
Enter
Sleep 6s

Enter
Sleep 100ms

Type 'envroll set DATABASE_URL=postgres://staging.db/app'
Enter
Sleep 6s

Enter
Sleep 100ms

Type 'envroll set STRIPE_KEY=sk_test_xyz'
Enter
Sleep 6s

Enter
Sleep 100ms

# `set` patches a single key inside the vault. `save` is the inverse —
# edit ./.env however you want (vim, code, sed, anything) and then tell
# envroll "snapshot the file as it is now". The intermediate `cat .env`
# makes the "look, I just changed the file on disk" beat visible.
Type 'echo "FEATURE_FLAG=on" >> .env'
Enter
Sleep 700ms

Type 'cat .env'
Enter
Sleep 2200ms

Type 'envroll save -m "enable feature flag"'
Enter
Sleep 6s

Enter
Sleep 100ms

# ---- Act 4: the killer — atomic switch ----
Type 'envroll use dev'
Enter
Sleep 5s

Type 'cat .env'
Enter
Sleep 2200ms

Enter
Sleep 100ms

Type 'envroll use staging'
Enter
Sleep 5s

Type 'cat .env'
Enter
Sleep 2500ms

Enter
Sleep 100ms

# ---- Act 4b: exec — run a command with a different env's vars without
#             changing ./.env. Note that printenv prints DEV's value while
#             ./.env on disk still has STAGING's. exec is per-process,
#             not a switch.
Type 'envroll exec dev -- printenv DATABASE_URL'
Enter
Sleep 5s

Enter
Sleep 100ms

# ---- Act 5: visualize history ----
Type 'envroll diff dev staging'
Enter
Sleep 6s

Enter
Sleep 100ms

Type 'envroll log staging'
Enter
Sleep 15s

Enter
Sleep 100ms

Type 'envroll list'
Enter
Sleep 6s

# Hold the final frame
Sleep 8s
