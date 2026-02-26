#!/usr/bin/env bash
set -euo pipefail

# ─── Benchmark: heft vs duc ──────────────────────────────────────────────
# Usage: sudo ./bench.sh <path_to_index> [runs]

TARGET="${1:?Usage: sudo ./bench.sh <path> [runs]}"
RUNS="${2:-3}"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
HEFT="$SCRIPT_DIR/target/release/heft"
DUC="$(command -v duc || true)"
HEFT_DB="/tmp/.heft-bench.db"
DUC_DB="/tmp/.duc-bench.db"

if [[ -z "$DUC" ]]; then
    echo "error: duc not found in PATH" >&2
    exit 1
fi
if [[ ! -x "$HEFT" ]]; then
    echo "Building heft (release)..."
    (cd "$SCRIPT_DIR" && cargo build --release --quiet)
fi

TARGET=$(realpath "$TARGET")

echo ""
echo "  heft vs duc benchmark"
echo "  ---------------------"
echo "  Target:  $TARGET"
echo "  Runs:    $RUNS"

# ─── Run benchmarks ──────────────────────────────────────────────────
declare -a DUC_TIMES HEFT_TIMES

for i in $(seq 1 "$RUNS"); do
    printf "\r  Run %d/%d ..." "$i" "$RUNS" >&2

    rm -f "$DUC_DB"
    DUC_START=$(date +%s%N)
    "$DUC" index -d "$DUC_DB" "$TARGET" 2>/dev/null
    DUC_END=$(date +%s%N)
    DUC_TIMES+=( $(( (DUC_END - DUC_START) / 1000000 )) )

    rm -rf "$HEFT_DB"
    HEFT_START=$(date +%s%N)
    "$HEFT" -d "$HEFT_DB" index "$TARGET" 2>/dev/null
    HEFT_END=$(date +%s%N)
    HEFT_TIMES+=( $(( (HEFT_END - HEFT_START) / 1000000 )) )
done
printf "\r                    \r" >&2

# ─── Compare stats ───────────────────────────────────────────────────
# duc info -b: "Date       Time       Files    Dirs    Size Path"
DUC_INFO=$("$DUC" info -b -d "$DUC_DB" 2>/dev/null | tail -1)
DUC_FILES=$(echo "$DUC_INFO" | awk '{print $3}')
DUC_DIRS=$(echo "$DUC_INFO" | awk '{print $4}')
DUC_BYTES=$(echo "$DUC_INFO" | awk '{print $5}')

# heft info: "/path  N files  N dirs  SIZE"
HEFT_INFO=$("$HEFT" -d "$HEFT_DB" info 2>/dev/null | head -1)
HEFT_FILES=$(echo "$HEFT_INFO" | grep -oP '[0-9,]+(?= files)')
HEFT_DIRS=$(echo "$HEFT_INFO" | grep -oP '[0-9,]+(?= dirs)')
HEFT_SIZE=$(echo "$HEFT_INFO" | grep -oP '[0-9.]+ [TGMK]iB|[0-9]+ B')

# Strip commas for numeric comparison.
HEFT_FILES_N=${HEFT_FILES//,/}
HEFT_DIRS_N=${HEFT_DIRS//,/}

printf "  %-6s  %10s files  %10s dirs  %s\n" "duc" "$DUC_FILES" "$DUC_DIRS" "$DUC_BYTES B"
printf "  %-6s  %10s files  %10s dirs  %s\n" "heft" "$HEFT_FILES" "$HEFT_DIRS" "$HEFT_SIZE"

STATS_OK=true
if [[ "$HEFT_FILES_N" != "$DUC_FILES" ]]; then
    echo "  ⚠  File count mismatch!"
    STATS_OK=false
fi
if [[ "$HEFT_DIRS_N" != "$DUC_DIRS" ]]; then
    echo "  ⚠  Dir count mismatch!"
    STATS_OK=false
fi
if $STATS_OK; then
    echo "  ✓  Stats match"
fi
echo ""

duc_sum=0; heft_sum=0
for t in "${DUC_TIMES[@]}"; do duc_sum=$((duc_sum + t)); done
for t in "${HEFT_TIMES[@]}"; do heft_sum=$((heft_sum + t)); done
DUC_AVG=$((duc_sum / RUNS))
HEFT_AVG=$((heft_sum / RUNS))

if (( HEFT_AVG > 0 && DUC_AVG > 0 )); then
    SPEEDUP=$(awk "BEGIN { printf \"%.2f\", $DUC_AVG / $HEFT_AVG }")
else
    SPEEDUP="N/A"
fi

DUC_DB_SIZE=$(du -sh "$DUC_DB" 2>/dev/null | cut -f1)
HEFT_DB_SIZE=$(du -sh "$HEFT_DB" 2>/dev/null | cut -f1)

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

# heft row
row="$(printf "  %-6s |" "heft")"
for t in "${HEFT_TIMES[@]}"; do row+="$(printf " %5dms" "$t")"; done
row+="  | $(printf "%5dms" "$HEFT_AVG")"
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
printf "  heft   %s\n" "$HEFT_DB_SIZE"
echo ""

rm -rf "$HEFT_DB" "$DUC_DB"
