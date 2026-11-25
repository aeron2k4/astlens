#!/usr/bin/env bash
# Table test for astlens-guard.sh and `astlens guard`; run from anywhere. Needs jq.
set -u
SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REPO_DIR=$(cd "$SCRIPT_DIR/.." && pwd)
GUARD=$SCRIPT_DIR/astlens-guard.sh
if [ -n "${ASTLENS_TEST_BIN:-}" ]; then
  BIN=$ASTLENS_TEST_BIN
else
  CARGO_BIN=$(command -v cargo || true)
  [ -n "$CARGO_BIN" ] || CARGO_BIN="$HOME/.cargo/bin/cargo"
  "$CARGO_BIN" build --quiet --locked --manifest-path "$REPO_DIR/Cargo.toml" || exit 1
  BIN="$REPO_DIR/target/debug/astlens"
fi
WS=$(mktemp -d)
OUTSIDE=$(mktemp -d)
trap 'rm -rf "$WS" "$OUTSIDE"' EXIT
mkdir -p "$WS/src"
echo 'fn main() {}' >"$WS/src/main.rs"
ln -s src/main.rs "$WS/notes.md"
ln -s notes.md "$WS/alias"
ln -s "$WS/src/main.rs" "$OUTSIDE/notes.md"
ln -s "$WS" "$OUTSIDE/project"
echo 'fn dependency() {}' >"$OUTSIDE/dependency.rs"
ln -s "$OUTSIDE/dependency.rs" "$WS/dependency.md"
ln -s README.md "$WS/alias.rs"
echo 'fn a() {}' >"$WS/src/a.rs"
echo 'fn b() {}' >"$WS/src/tiếng việt.rs"
echo '# hi' >"$WS/README.md"
echo '# hi' >"$WS/NOTES.MD"
echo 'fn up() {}' >"$WS/src/UP.RS"
echo 'int x;' >"$WS/src/x.c++"
echo 'int y;' >"$WS/src/y.H++"
echo 'int i;' >"$WS/src/z.inl"
echo 'int z;' >"$WS/src/z.ipp"
echo 'int c;' >"$WS/src/t.tcc"
echo 'int t;' >"$WS/src/t.tpp"
echo 'func p():' >"$WS/src/player.GD"
echo 'def a; end' >"$WS/src/cart.rb"
echo 'Gem::Specification.new' >"$WS/src/shop.GEMSPEC"
echo 'run App' >"$WS/config.RU"
echo 'json.a 1' >"$WS/src/show.jbuilder"
echo 'robe' >"$WS/src/robe.txt"
fail=0

# verdict <mode> <tool> <json tool_input>
verdict() {
  local mode=$1 payload json
  payload=$(jq -n --arg t "$2" --arg c "$WS" --argjson i "$3" '{tool_name: $t, cwd: $c, tool_input: $i}')
  if [ "$mode" = "sh" ]; then
    json=$(bash "$GUARD" <<<"$payload") || {
      printf 'FAIL sh guard crashed on: %s\n' "$3" >&2
      fail=1
      return
    }
  else
    json=$("$BIN" guard <<<"$payload") || {
      printf 'FAIL bin guard crashed on: %s\n' "$3" >&2
      fail=1
      return
    }
  fi
  jq -r '.hookSpecificOutput.permissionDecision // empty' <<<"$json" || {
    printf 'FAIL %s guard printed bad JSON on: %s\n' "$mode" "$3" >&2
    fail=1
  }
}

expect() {
  local want=$1 tool=$2 json=$3 label=$4 got mode
  for mode in sh bin; do
    got=$(verdict "$mode" "$tool" "$json")
    [ -n "$got" ] || got=allow
    if [ "$got" != "$want" ]; then
      printf 'FAIL (%s) %s: want %s, got %s\n' "$mode" "$label" "$want" "$got"
      fail=1
    fi
  done
}

bash_cmd() { expect "$1" Bash "$(jq -n --arg c "$2" '{command: $c}')" "$2"; }

expect deny Read "{\"file_path\": \"$WS/src/main.rs\"}" 'Read whole file'
expect deny Read "{\"file_path\": \"$WS/notes.md\"}" 'Read source through symlink'
expect deny Read "{\"file_path\": \"$WS/alias\"}" 'Read source through symlink chain without extension'
expect deny Read "{\"file_path\": \"$OUTSIDE/notes.md\"}" 'Read outside alias of workspace source'
expect deny Read "{\"file_path\": \"$OUTSIDE/project/src/main.rs\"}" 'Read through workspace directory alias'
expect allow Read "{\"file_path\": \"$WS/notes.md\", \"limit\": 50}" 'Read limited through source alias'
expect allow Read "{\"file_path\": \"$WS/dependency.md\"}" 'Read alias of outside source'
expect deny Read "{\"file_path\": \"$WS/alias.rs\"}" 'Read source extension alias of non-code file'
expect deny Grep '{"pattern": "x", "path": "notes.md"}' 'Grep source alias'
bash_cmd deny "cat $OUTSIDE/notes.md"
bash_cmd deny "grep foo $OUTSIDE/notes.md"
expect allow Read "{\"file_path\": \"$WS/src/main.rs\", \"limit\": 50}" 'Read limited'
expect allow Read '{"file_path": "/etc/x.rs"}' 'Read outside workspace'
expect deny Read "{\"file_path\": \"$WS/src/UP.RS\"}" 'Read upper-case extension'
expect deny Read "{\"file_path\": \"$WS/src/x.c++\"}" 'Read .c++'
expect deny Read "{\"file_path\": \"$WS/src/z.ipp\"}" 'Read .ipp'
expect deny Read "{\"file_path\": \"$WS/src/player.GD\"}" 'Read .gd'
expect deny Grep '{"pattern": "x", "type": "gdscript"}' 'Grep gdscript type'
expect deny Read "{\"file_path\": \"$WS/src/cart.rb\"}" 'Read .rb'
expect deny Read "{\"file_path\": \"$WS/src/shop.GEMSPEC\"}" 'Read .gemspec'
expect deny Read "{\"file_path\": \"$WS/config.RU\"}" 'Read .ru'
expect deny Read "{\"file_path\": \"$WS/src/show.jbuilder\"}" 'Read .jbuilder'
expect deny Grep '{"pattern": "x", "type": "ruby"}' 'Grep ruby type'
expect deny Grep '{"pattern": "x", "glob": "*.rake"}' 'Grep .rake glob'
expect allow Read "{\"file_path\": \"$WS/src/robe.txt\"}" 'Read .txt with letters of rb'
expect allow Read "{\"file_path\": \"$WS/NOTES.MD\"}" 'Read upper-case non-code'
expect deny Grep '{"pattern": "x", "glob": "*.RS"}' 'Grep upper-case glob'
expect deny Grep '{"pattern": "x", "glob": "*.c++"}' 'Grep .c++ glob'
expect deny Grep '{"pattern": "x", "glob": "*.INL"}' 'Grep .INL glob'
expect allow Grep '{"pattern": "x", "glob": "*.MD"}' 'Grep upper-case non-code glob'
bash_cmd deny 'cat src/UP.RS'
bash_cmd deny 'cat src/x.c++'
bash_cmd deny 'head -n 5000 src/y.H++'
bash_cmd deny 'cat src/z.inl'
bash_cmd deny 'sed -n 1,5000p src/t.tcc'
bash_cmd deny 'sed -n 1,5000p src/t.tpp'
bash_cmd allow 'head src/y.H++'
bash_cmd allow 'head src/main.rs'
bash_cmd deny 'grep foo src/UP.RS'
bash_cmd deny 'grep --include=*.c++ -r foo .'
bash_cmd allow 'cat NOTES.MD'
bash_cmd allow 'cat src'

while IFS= read -r c; do [ -n "$c" ] && bash_cmd deny "$c"; done <<'CASES'
grep -rn foo src
git -C . grep .
git --no-pager -C . grep .
rtk grep foo src
cat src/main.rs
cat notes.md
cat alias
cat notes.*
cat alias.rs
grep foo notes.md
cat src/main.rs >/tmp/copy
cat src/main.rs>/tmp/copy
cat src/main.rs >>/tmp/copy
cat src/main.rs 1>>/tmp/copy
cat src/main.rs >/dev/stdout
cat src/main.rs > /dev/stdout
cat src/main.rs >/tmp/copy | wc -l
cat src/main.rs > /tmp/copy | head -n 50
cat<src/main.rs>/tmp/copy
cat -n src/main.rs
cat README.md src/main.rs
cat src/*
cat src/*.rs
cat src/*.{rs,py}
cat < src/main.rs
cat src/main.rs 2>/dev/null
cd src && cat main.rs
if true; then cat src/main.rs; fi
tac src/main.rs
nl src/main.rs
less src/main.rs
more src/main.rs
bat src/main.rs
rtk read src/main.rs
rtk read -n src/main.rs
rtk read --head-lines 2000 src/main.rs
rtk bypass cat src/main.rs
rtk smart src/main.rs
sed -n 1,400p src/main.rs
sed -n /foo/p src/main.rs
sed s/a/b/ src/main.rs
sed 1,5p src/main.rs
head -n 2000 src/main.rs
head -2000 src/main.rs
head -c 100000 src/main.rs
head -n -5 src/main.rs
tail -n +5 src/main.rs
awk 1 src/main.rs
awk -F: '{print $1}' src/main.rs
cat src/main.rs | grep foo
cat src/main.rs | head -n 2000
cat src/main.rs | less
"cat" "src/main.rs"
/bin/cat src/main.rs
cat "src/tiếng việt.rs"
sed 999q src/main.rs
| cat src/main.rs
| cat src/main.rs |
CASES

while IFS= read -r c; do [ -n "$c" ] && bash_cmd allow "$c"; done <<'CASES'
cat README.md
cat dependency.md
head -n 50 notes.md
cat /etc/hostname
cat src
cat README*
cat nosuch.rs
cat src/main.rs | wc -l
cat src/main.rs | rtk wc -l
cat src/main.rs | sha256sum
cat src/main.rs | head -n 50
cat src/main.rs | sed -n 1,50p
cat src/main.rs > /dev/null
cat src/main.rs >/dev/null
cat src/main.rs>/dev/null
cat src/main.rs 1>>/dev/null
cat src/main.rs >/dev/null 2>&1
head -n 50 src/main.rs >/tmp/copy
cat > src/new.rs
cat >> src/main.rs
head src/main.rs
head -n 50 src/main.rs
head -50 src/main.rs
tail -n 50 src/main.rs
sed -n 1,50p src/main.rs
sed -n '10,20p' src/main.rs
sed -n 5p src/main.rs
sed -i s/a/b/ src/main.rs
sed -n 1,400p src/main.rs | wc -l
rtk read --head-lines 50 src/main.rs
rtk read -m 100 src/main.rs
rtk read README.md
read line
smart src/main.rs
ls | head -5
ls | awk '{print $1*2}'
ls | sed s/a*/b/
echo cat src/main.rs
echo src/main.rs | head -1
git log --oneline | head -5
cargo test 2>&1 | tail -20
sed 10q src/main.rs
concat src/main.rs
head-to-head src/main.rs
cargo build src/main.rs
head -n 50 "src/tiếng việt.rs"
CASES

# Heredoc bodies are data: the commands inside must not count.
bash_cmd allow "cat > notes.md <<'END'
cat src/main.rs
grep -rn foo src
END"
bash_cmd deny "cat > notes.md <<END
text
END
cat src/main.rs"
bash_cmd deny "cat > notes.md <<END
dòng tiếng Việt với *
END
cat src/main.rs"
bash_cmd allow "cat <<-END
	cat src/main.rs
	END"
bash_cmd allow "git commit -m \"\$(cat <<'END'
cat src/main.rs
END
)\""

# .. never leaves or re-enters the workspace by spelling alone.
expect allow Read "{\"file_path\": \"$WS/src/../../elsewhere/x.rs\"}" 'Read .. outside'
expect deny Read "{\"file_path\": \"/nowhere/..$WS/src/main.rs\"}" 'Read .. back inside'
bash_cmd allow "cat ../../x.rs"

# OpenCode's shell starts in workdir; the workspace stays cwd.
workdir_cmd() {
  expect "$1" Bash "$(jq -n --arg c "$2" --arg w "$3" '{command: $c, workdir: $w}')" "$2 (workdir $3)"
}
workdir_cmd deny "cat $WS/src/main.rs" /tmp
workdir_cmd deny "cat main.rs" src
workdir_cmd deny "cat main.rs" "$WS/src"
workdir_cmd allow "cat main.rs" /tmp
workdir_cmd allow "cat /etc/x.rs" /

# OpenCode gets its own tool names in the denial reason.
for runner in "bash $GUARD" "$BIN guard"; do
  reason=$(jq -n --arg c "$WS" \
    '{tool_name: "Read", cwd: $c, tool_input: {file_path: "src/main.rs"}}' |
    ASTLENS_OPENCODE=1 $runner | jq -r '.hookSpecificOutput.permissionDecisionReason // empty')
  case "$reason" in
    *astlens_read_symbol*) ;;
    *) printf 'FAIL OpenCode hint (%s): %s\n' "$runner" "$reason"; fail=1 ;;
  esac
done

# Native Rust Antigravity guard tracks workspace root and enforces PreToolUse without python3.
mkdir -p "$WS/.cache/astlens"
agy_verdict() {
  local tool=$1 args_json=$2
  ASTLENS_CACHE_DIR="$WS/.cache/astlens" jq -n --arg c "$WS" --arg t "$tool" --argjson a "$args_json" \
    '{hookEventName: "PreToolUse", cwd: $c, toolCall: {name: $t, args: $a}}' |
    ASTLENS_CACHE_DIR="$WS/.cache/astlens" "$BIN" guard --client antigravity
}

out=$(agy_verdict read_file "{\"Path\": \"$WS/src/main.rs\"}")
[ "$(jq -r '.decision' <<<"$out")" = "deny" ] || { printf 'FAIL agy whole read: %s\n' "$out"; fail=1; }
case "$(jq -r '.reason' <<<"$out")" in
  *call_mcp_tool*) ;;
  *) printf 'FAIL agy reason: %s\n' "$out"; fail=1 ;;
esac

out=$(agy_verdict read_file "{\"Path\": \"$WS/src/main.rs\", \"StartLine\": 1, \"EndLine\": 50}")
[ "$(jq -r '.decision' <<<"$out")" = "allow" ] || { printf 'FAIL agy bounded read: %s\n' "$out"; fail=1; }

out=$(agy_verdict run_command '{"CommandLine": "cat src/main.rs"}')
[ "$(jq -r '.decision' <<<"$out")" = "deny" ] || { printf 'FAIL agy cat: %s\n' "$out"; fail=1; }

got_root=$(cat "$WS/.cache/astlens/antigravity_root" 2>/dev/null || true)
want_root=$(cd "$WS" && pwd -P)
[ "$got_root" = "$want_root" ] || { printf 'FAIL agy root cache: want %s, got %s\n' "$want_root" "$got_root"; fail=1; }

((fail)) && exit 1
echo 'guard: all cases pass'
