#!/bin/bash
#
# just-do-it.sh - Run jdi workflow in a loop until completion
#
# Usage: ./just-do-it.sh [options]
#
# Options:
#   -m N   Maximum iterations (default: 10, 0 = unlimited)
#   -w NAME  Workflow name or path
#   -t ID    Specific task ID
#   -s       Stop after current task completes
#   -h       Show this help message
#
# Exit codes: 0=success, 1=abort, 2=max-iterations, 3=human-required
#

set -euo pipefail
export CLAUDE_CODE_ENABLE_TASKS=true

max=10 workflow="" task_id="" stop_on_complete=false

while [[ $# -gt 0 ]]; do
    case $1 in
        -m) max="$2"; shift 2 ;;
        -w) workflow="$2"; shift 2 ;;
        -t) task_id="$2"; shift 2 ;;
        -s) stop_on_complete=true; shift ;;
        -h) sed -n '2,/^$/p' "$0" | sed 's/^#//; s/^ //'; exit 0 ;;
        *)  echo "Unknown option: $1"; exit 1 ;;
    esac
done

cmd="/jdi run"
[[ -n "$workflow" ]] && cmd="$cmd --workflow $workflow"
[[ -n "$task_id" ]] && cmd="$cmd --task $task_id"

_outfile=$(mktemp)
_claude_pid=""
cleanup() { rm -f "$_outfile" 2>/dev/null; }
trap 'cleanup' EXIT
trap '
    echo ""
    echo "Interrupted"
    if [[ -n "$_claude_pid" ]]; then
        kill "$_claude_pid" 2>/dev/null
        kill -9 "$_claude_pid" 2>/dev/null
        wait "$_claude_pid" 2>/dev/null
    fi
    rm -f .jdi/locks/*.lock 2>/dev/null
    exit 130
' INT TERM

iteration=0
while true; do
    iteration=$((iteration + 1))
    if [[ $max -gt 0 ]] && [[ $iteration -gt $max ]]; then
        echo "Max iterations ($max) reached"
        exit 2
    fi

    echo "--- Iteration $iteration${max:+/$max} ---"

    claude_exit=0
    claude -p "$cmd" >"$_outfile" 2>&1 &
    _claude_pid=$!
    wait "$_claude_pid" || claude_exit=$?
    _claude_pid=""
    tail -20 "$_outfile"

    if [[ $claude_exit -eq 130 ]] || [[ $claude_exit -eq 143 ]]; then
        echo "Interrupted"
        rm -f .jdi/locks/*.lock 2>/dev/null
        exit 130
    fi

    rm -f .jdi/locks/*.lock 2>/dev/null

    status=$(awk '{print $1; exit}' .jdi/status 2>/dev/null || echo "ABORT")

    echo "Status: $status"

    case $status in
        CONTINUE)
            ;;
        STEP_COMPLETE)
            if [[ "$stop_on_complete" == true ]]; then
                echo "Task completed. Stopping (-s)."
                exit 0
            fi
            echo "Task completed. Continuing..."
            ;;
        WORKFLOW_COMPLETE)
            echo "All tasks completed — workflow finished!"
            exit 0
            ;;
        ABORT)
            echo "Workflow aborted! Check .jdi/reports/ for details."
            exit 1
            ;;
        HUMAN_REQUIRED)
            echo "Human step reached. Run '/jdi run --human' to continue, then restart."
            exit 3
            ;;
        *)
            echo "Unknown status: $status"
            exit 1
            ;;
    esac
done
