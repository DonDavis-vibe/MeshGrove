#!/usr/bin/env bash
# Compare our vendored SolidCraft code and assets with an upstream SolidCraft checkout.
#
#   scripts/sync-solidcraft.sh [status]        per file: same, ours (our patch), theirs (upstream
#                                          changed since our base), both (needs a merge), added
#                                          (only here), new (only upstream, in a vendored crate)
#   scripts/sync-solidcraft.sh patches         our patches: unified diff base -> ours
#   scripts/sync-solidcraft.sh upstream        upstream's changes since our base: base -> upstream HEAD
#   scripts/sync-solidcraft.sh merge           3-way merge upstream HEAD into ours (git merge-file),
#                                          copy new upstream files, then record the new base
#
# The base is the upstream commit we vendored from (vendor/UPSTREAM_COMMIT, first line). Upstream
# is $SOLIDCRAFT or /home/rec0il/Dokumente/Scritps/solidcraft; it is only read, never written.
set -euo pipefail

here="$(cd "$(dirname "$0")/.." && pwd)"
up="${SOLIDCRAFT:-/home/rec0il/Dokumente/Scritps/solidcraft}"
mode="${1:-status}"
base="$(head -n1 "$here/vendor/UPSTREAM_COMMIT")"
head_rev="$(git -C "$up" rev-parse HEAD)"

# Our path -> upstream path. Whole directories are vendored crates (new upstream files show);
# asset directories hold a chosen subset (only the files we have are compared).
crates=(color theme ui-text ui-registry ui-kit)
declare -a pairs=()
for c in "${crates[@]}"; do pairs+=("vendor/solidcraft-$c|crates/$c|crate"); done
pairs+=("assets/themes|assets/styles/ui/themes|subset")
pairs+=("assets/icons/palette.toml|assets/icons/ui/design/palette.toml|file")
pairs+=("assets/icons/aliases.toml|assets/icons/ui/design/aliases.toml|file")
pairs+=("assets/icons/svg|assets/icons/ui/design/svg|subset")
pairs+=("assets/fonts|assets/fonts|subset")
# Files we add to every vendored crate (the licence texts from upstream's root).
added_ok='^(LICENSE-MIT|LICENSE-APACHE|NOTICE)$'

at_base() { git -C "$up" show "$base:$1" 2>/dev/null; }
exists_at_base() { git -C "$up" cat-file -e "$base:$1" 2>/dev/null; }

# Every (ours|upstream) relative file pair to compare, as "ours_rel|up_rel|kind".
list_files() {
  for pair in "${pairs[@]}"; do
    IFS='|' read -r ours theirs kind <<<"$pair"
    case "$kind" in
      file) echo "$ours|$theirs|file" ;;
      crate|subset)
        (cd "$here/$ours" && find . -type f ! -path './target/*' | sed 's#^\./##' | sort) | while read -r f; do
          echo "$ours/$f|$theirs/$f|$kind"
        done
        if [[ "$kind" == crate ]]; then
          # Upstream files at HEAD that we do not have yet.
          (cd "$up/$theirs" && git ls-files | sort) | while read -r f; do
            [[ -e "$here/$ours/$f" ]] || echo "$ours/$f|$theirs/$f|new"
          done
        fi ;;
    esac
  done
}

classify() {
  local ours="$1" theirs="$2" kind="$3"
  if [[ "$kind" == new ]]; then echo new; return; fi
  if ! exists_at_base "$theirs"; then
    if [[ "$(basename "$ours")" =~ $added_ok ]] || [[ ! -e "$up/$theirs" ]]; then echo added; else echo theirs; fi
    return
  fi
  local o t
  o=$(at_base "$theirs" | cmp -s - "$here/$ours" && echo same || echo changed)
  if [[ -e "$up/$theirs" ]]; then
    t=$(at_base "$theirs" | cmp -s - "$up/$theirs" && echo same || echo changed)
  else
    t=deleted
  fi
  case "$o/$t" in
    same/same) echo same ;;
    changed/same) echo ours ;;
    same/changed) echo theirs ;;
    same/deleted) echo deleted ;;
    *) echo both ;;
  esac
}

echo "upstream: $up"
echo "base:     $base ($(git -C "$up" log -1 --format=%cs "$base" 2>/dev/null || echo unknown))"
echo "head:     $head_rev ($(git -C "$up" log -1 --format=%cs HEAD), branch $(git -C "$up" branch --show-current))"
if [[ -n "$(git -C "$up" status --porcelain -- crates/color crates/theme crates/ui-text crates/ui-registry crates/ui-kit assets/styles/ui/themes assets/icons/ui/design assets/fonts)" ]]; then
  echo "warning:  upstream has uncommitted changes in vendored paths; HEAD comparisons use the working tree"
fi
echo

case "$mode" in
  status)
    declare -A count=()
    while IFS='|' read -r ours theirs kind; do
      state=$(classify "$ours" "$theirs" "$kind")
      count[$state]=$(( ${count[$state]:-0} + 1 ))
      [[ "$state" == same ]] || printf '%-8s %s\n' "$state" "$ours"
    done < <(list_files)
    echo
    for k in same ours theirs both added new deleted; do printf '%s=%s ' "$k" "${count[$k]:-0}"; done; echo ;;
  patches)
    while IFS='|' read -r ours theirs kind; do
      state=$(classify "$ours" "$theirs" "$kind")
      if [[ "$state" == ours || "$state" == both ]]; then
        diff -u --label "upstream@${base:0:9}:$theirs" --label "$ours" <(at_base "$theirs") "$here/$ours" || true
      fi
    done < <(list_files) ;;
  upstream)
    while IFS='|' read -r ours theirs kind; do
      state=$(classify "$ours" "$theirs" "$kind")
      if [[ "$state" == theirs || "$state" == both ]]; then
        diff -u --label "upstream@${base:0:9}:$theirs" --label "upstream@HEAD:$theirs" <(at_base "$theirs") "$up/$theirs" || true
      elif [[ "$state" == new || "$state" == deleted ]]; then
        echo "$state upstream: $theirs"
      fi
    done < <(list_files) ;;
  merge)
    conflicts=0
    tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT
    while IFS='|' read -r ours theirs kind; do
      state=$(classify "$ours" "$theirs" "$kind")
      case "$state" in
        theirs) cp "$up/$theirs" "$here/$ours"; echo "updated  $ours" ;;
        new) mkdir -p "$(dirname "$here/$ours")"; cp "$up/$theirs" "$here/$ours"; echo "new      $ours" ;;
        deleted) echo "upstream deleted $theirs: remove $ours by hand if we no longer need it" ;;
        both)
          at_base "$theirs" > "$tmp/base"
          if git merge-file -L ours -L "base ${base:0:9}" -L "upstream ${head_rev:0:9}" "$here/$ours" "$tmp/base" "$up/$theirs"; then
            echo "merged   $ours"
          else
            echo "CONFLICT $ours"; conflicts=$((conflicts + 1))
          fi ;;
      esac
    done < <(list_files)
    if (( conflicts == 0 )); then
      printf '%s\n%s\n' "$head_rev" "$(git -C "$up" log -1 --format=%cd HEAD)" > "$here/vendor/UPSTREAM_COMMIT"
      echo; echo "base is now $head_rev. Update vendor/PATCHES.md, then build and test."
    else
      echo; echo "$conflicts conflict(s): resolve the markers, then set vendor/UPSTREAM_COMMIT to $head_rev by hand."
      exit 1
    fi ;;
  *) echo "unknown mode $mode (status, patches, upstream, merge)"; exit 2 ;;
esac
