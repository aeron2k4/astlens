#!/usr/bin/env bash
# Table test for astlens-guard.sh; run from anywhere. Needs jq.
set -u
GUARD=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/astlens-guard.sh
WS=$(mktemp -d)
trap 'rm -rf "$WS"' EXIT
mkdir -p "$WS/src"
echo 'fn main() {}' >"$WS/src/main.rs"
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

# verdict <tool> <json tool_input>
verdict() {
  local json
  json=$(jq -n --arg t "$1" --arg c "$WS" --argjson i "$2" \
    '{tool_name: $t, cwd: $c, tool_input: $i}' | bash "$GUARD") || {
    printf 'FAIL guard crashed on: %s\n' "$2" >&2
    fail=1
    return
  }
  jq -r '.hookSpecificOutput.permissionDecision // empty' <<<"$json" || {
    printf 'FAIL guard printed no JSON on: %s\n' "$2" >&2
    fail=1
  }
}

expect() {
  local want=$1 tool=$2 json=$3 label=$4 got
  got=$(verdict "$tool" "$json")
  [ -n "$got" ] || got=allow
  if [ "$got" != "$want" ]; then
    printf 'FAIL %s: want %s, got %s\n' "$label" "$want" "$got"
    fail=1
  fi
}

bash_cmd() { expect "$1" Bash "$(jq -n --arg c "$2" '{command: $c}')" "$2"; }

expect deny Read "{\"file_path\": \"$WS/src/main.rs\"}" 'Read whole file'
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
rtk grep foo src
cat src/main.rs
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
cat src/main.rs >/tmp/copy
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
reason=$(jq -n --arg c "$WS" \
  '{tool_name: "Read", cwd: $c, tool_input: {file_path: "src/main.rs"}}' |
  ASTLENS_OPENCODE=1 bash "$GUARD" | jq -r '.hookSpecificOutput.permissionDecisionReason // empty')
case "$reason" in
  *astlens_read_symbol*) ;;
  *) printf 'FAIL OpenCode hint: %s\n' "$reason"; fail=1 ;;
esac

# Antigravity adapter translates PreToolUse events and records workspace root.
if command -v python3 >/dev/null 2>&1; then
  AGY_DIR="$WS/agy-hooks"
  mkdir -p "$AGY_DIR" "$WS/.cache"
  cp "$GUARD" "$AGY_DIR/astlens-guard.sh"
  cp "$(dirname "$GUARD")/agy-guard.py" "$AGY_DIR/agy-guard.py"

  agy_verdict() {
    local tool=$1 args_json=$2
    XDG_CACHE_HOME="$WS/.cache" jq -n --arg c "$WS" --arg t "$tool" --argjson a "$args_json" \
      '{hookEventName: "PreToolUse", cwd: $c, toolCall: {name: $t, args: $a}}' |
      XDG_CACHE_HOME="$WS/.cache" python3 "$AGY_DIR/agy-guard.py"
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
  [ "$got_root" = "$WS" ] || { printf 'FAIL agy root cache: want %s, got %s\n' "$WS" "$got_root"; fail=1; }
fi

((fail)) && exit 1
echo 'guard: all cases pass'
