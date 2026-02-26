#!/usr/bin/env bash
set -euo pipefail

# ─── Benchmark: dux vs duc ──────────────────────────────────────────────
# Usage: sudo ./bench.sh <path_to_index> [runs]

TARGET="${1:?Usage: sudo ./bench.sh <path> [runs]}"
RUNS="${2:-3}"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
DUX="$SCRIPT_DIR/target/release/dux"
DUC="$(command -v duc || true)"
DUX_DB="/tmp/.dux-bench.db"
DUC_DB="/tmp/.duc-bench.db"

if [[ -z "$DUC" ]]; then
    echo "error: duc not found in PATH" >&2
    exit 1
fi
if [[ ! -x "$DUX" ]]; then
    echo "Building dux (release)..."
    (cd "$SCRIPT_DIR" && cargo build --release --quiet)
fi

TARGET=$(realpath "$TARGET")

echo ""
echo "  dux vs duc benchmark"
echo "  --------------------"
echo "  Target:  $TARGET"
echo "  Runs:    $RUNS"

# ─── First run: warm up and capture stats ─────────────────────────────
declare -a DUC_TIMES DUX_TIMES

rm -f "$DUC_DB"
DUC_START=$(date +%s%N)
"$DUC" index -d "$DUC_DB" "$TARGET" 2>/dev/null
DUC_END=$(date +%s%N)
DUC_TIMES+=( $(( (DUC_END - DUC_START) / 1000000 )) )

rm -rf "$DUX_DB"
DUX_START=$(date +%s%N)
DUX_SUMMARY=$("$DUX" -d "$DUX_DB" index "$TARGET" 2>&1 >/dev/null)
DUX_END=$(date +%s%N)
DUX_TIMES+=( $(( (DUX_END - DUX_START) / 1000000 )) )

# ─── Compare stats ───────────────────────────────────────────────────
# duc info -b: "Date       Time       Files    Dirs    Size Path"
DUC_INFO=$("$DUC" info -b -d "$DUC_DB" 2>/dev/null | tail -1)
DUC_FILES=$(echo "$DUC_INFO" | awk '{print $3}')
DUC_DIRS=$(echo "$DUC_INFO" | awk '{print $4}')
DUC_BYTES=$(echo "$DUC_INFO" | awk '{print $5}')

# dux info: "/path  N files  N dirs  SIZE"
DUX_INFO=$("$DUX" -d "$DUX_DB" info 2>/dev/null | head -1)
DUX_FILES=$(echo "$DUX_INFO" | grep -oP '[0-9,]+(?= files)')
DUX_DIRS=$(echo "$DUX_INFO" | grep -oP '[0-9,]+(?= dirs)')
DUX_SIZE=$(echo "$DUX_INFO" | grep -oP '[0-9.]+ [TGMK]iB|[0-9]+ B')

# Strip commas for numeric comparison.
DUX_FILES_N=${DUX_FILES//,/}
DUX_DIRS_N=${DUX_DIRS//,/}

printf "  %-6s  %10s files  %10s dirs  %s\n" "duc" "$DUC_FILES" "$DUC_DIRS" "$DUC_BYTES B"
printf "  %-6s  %10s files  %10s dirs  %s\n" "dux" "$DUX_FILES" "$DUX_DIRS" "$DUX_SIZE"

STATS_OK=true
if [[ "$DUX_FILES_N" != "$DUC_FILES" ]]; then
    echo "  ⚠  File count mismatch!"
    STATS_OK=false
fi
if [[ "$DUX_DIRS_N" != "$DUC_DIRS" ]]; then
    echo "  ⚠  Dir count mismatch!"
    STATS_OK=false
fi
if $STATS_OK; then
    echo "  ✓  Stats match"
fi
echo ""

# ─── Remaining runs ───────────────────────────────────────────────────
for i in $(seq 2 "$RUNS"); do
    rm -f "$DUC_DB"
    DUC_START=$(date +%s%N)
    "$DUC" index -d "$DUC_DB" "$TARGET" 2>/dev/null
    DUC_END=$(date +%s%N)
    DUC_TIMES+=( $(( (DUC_END - DUC_START) / 1000000 )) )

    rm -rf "$DUX_DB"
    DUX_START=$(date +%s%N)
    "$DUX" -d "$DUX_DB" index "$TARGET" 2>/dev/null
    DUX_END=$(date +%s%N)
    DUX_TIMES+=( $(( (DUX_END - DUX_START) / 1000000 )) )
done

duc_sum=0; dux_sum=0
for t in "${DUC_TIMES[@]}"; do duc_sum=$((duc_sum + t)); done
for t in "${DUX_TIMES[@]}"; do dux_sum=$((dux_sum + t)); done
DUC_AVG=$((duc_sum / RUNS))
DUX_AVG=$((dux_sum / RUNS))

if (( DUX_AVG > 0 && DUC_AVG > 0 )); then
    SPEEDUP=$(awk "BEGIN { printf \"%.2f\", $DUC_AVG / $DUX_AVG }")
else
    SPEEDUP="N/A"
fi

DUC_DB_SIZE=$(du -sh "$DUC_DB" 2>/dev/null | cut -f1)
DUX_DB_SIZE=$(du -sh "$DUX_DB" 2>/dev/null | cut -f1)

# ─── Build table ─────────────────────────────────────────────────────────
COL=8

# Header line
hdr="$(printf "  %-6s |" "")"
for i in $(seq 1 "$RUNS"); do hdr+="$(printf " %6s" "#$i")"; done
hdr+="  | $(printf "%7s" "avg")"
LINE_W=${#hdr}
bar="  $(printf '%*s' $((LINE_W - 2)) '' | tr ' ' '-')"

echo "  INDEX SPEED"
echo "$bar"
echo "$hdr"
echo "$bar"

# duc row
row="$(printf "  %-6s |" "duc")"
for t in "${DUC_TIMES[@]}"; do row+="$(printf " %5dms" "$t")"; done
row+="  | $(printf "%5dms" "$DUC_AVG")"
echo "$row"

# dux row
row="$(printf "  %-6s |" "dux")"
for t in "${DUX_TIMES[@]}"; do row+="$(printf " %5dms" "$t")"; done
row+="  | $(printf "%5dms" "$DUX_AVG")"
echo "$row"

echo "$bar"

# speedup row
row="$(printf "  %-6s |" "")"
for i in $(seq 1 "$RUNS"); do row+="$(printf " %6s" "")"; done
row+="  | $(printf "%7s" "${SPEEDUP}x")"
echo "$row"

echo ""
echo "  DATABASE SIZE"
echo "  ------------------"
printf "  duc    %s\n" "$DUC_DB_SIZE"
printf "  dux    %s\n" "$DUX_DB_SIZE"
echo ""

rm -rf "$DUX_DB" "$DUC_DB"
