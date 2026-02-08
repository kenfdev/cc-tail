#!/bin/bash
#
# just-do-it.sh - Run jdi workflow in a loop until completion
#
# Usage:
#   ./just-do-it.sh [options]
#
# Options:
#   -m, --max-iterations N   Maximum iterations (default: 10, 0 = unlimited)
#   -w, --workflow NAME      Workflow name or path (optional)
#   -t, --task ID            Specific task ID (optional)
#   -s, --stop-on-complete   Stop after current task completes
#   -v, --verbose            Show detailed output
#   -h, --help               Show this help message
#
# Status values from /jdi run:
#   CONTINUE           - More steps remain for current task; keep looping
#   STEP_COMPLETE      - A task's workflow finished; loop continues to next task
#   WORKFLOW_COMPLETE   - No more tasks available; loop exits successfully
#   ABORT              - Critical error; loop exits with failure
#   HUMAN_REQUIRED     - Human approval needed; loop pauses
#
# Examples:
#   ./just-do-it.sh                          # Run with defaults (max 10 iterations)
#   ./just-do-it.sh -m 50                    # Run with max 50 iterations
#   ./just-do-it.sh -m 0                     # Run unlimited until WORKFLOW_COMPLETE/ABORT
#   ./just-do-it.sh -w code-review -t 123    # Run specific workflow and task
#   ./just-do-it.sh -s                       # Stop after first task completes
#

set -euo pipefail

# Default configuration
export CLAUDE_CODE_ENABLE_TASKS=true
MAX_ITERATIONS=10
WORKFLOW=""
TASK_ID=""
STOP_ON_COMPLETE=false
VERBOSE=false
STATUS_FILE=".jdi/status"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Print colored output
log_info()  { echo -e "${BLUE}[INFO]${NC} $1"; }
log_ok()    { echo -e "${GREEN}[OK]${NC} $1"; }
log_warn()  { echo -e "${YELLOW}[WARN]${NC} $1"; }
log_error() { echo -e "${RED}[ERROR]${NC} $1"; }

# Show help
show_help() {
    sed -n '2,/^$/p' "$0" | sed 's/^#//; s/^ //'
    exit 0
}

# Parse command line arguments
parse_args() {
    while [[ $# -gt 0 ]]; do
        case $1 in
            -m|--max-iterations)
                MAX_ITERATIONS="$2"
                shift 2
                ;;
            -w|--workflow)
                WORKFLOW="$2"
                shift 2
                ;;
            -t|--task)
                TASK_ID="$2"
                shift 2
                ;;
            -s|--stop-on-complete)
                STOP_ON_COMPLETE=true
                shift
                ;;
            -v|--verbose)
                VERBOSE=true
                shift
                ;;
            -h|--help)
                show_help
                ;;
            *)
                log_error "Unknown option: $1"
                echo "Use -h or --help for usage information."
                exit 1
                ;;
        esac
    done
}

# Build the jdi command
build_command() {
    local cmd="/jdi run"

    [[ -n "$WORKFLOW" ]] && cmd="$cmd --workflow $WORKFLOW"
    [[ -n "$TASK_ID" ]] && cmd="$cmd --task $TASK_ID"

    echo "$cmd"
}

# Read status from file (format: STATUS or STATUS step=name)
read_status() {
    if [[ -f "$STATUS_FILE" ]]; then
        cat "$STATUS_FILE" 2>/dev/null || echo "ABORT"
    else
        echo "ABORT"
    fi
}

# Extract the base status (first word) from the status line
parse_status() {
    echo "$1" | awk '{print $1}'
}

# Extract step name from status line (e.g., "STEP_COMPLETE step=finalize" → "finalize")
parse_step_name() {
    local step_field
    step_field=$(echo "$1" | grep -o 'step=[^ ]*' || true)
    if [[ -n "$step_field" ]]; then
        echo "${step_field#step=}"
    fi
}

# Main execution loop
main() {
    parse_args "$@"

    local cmd
    cmd=$(build_command)

    log_info "Starting jdi loop"
    log_info "Max iterations: ${MAX_ITERATIONS:-unlimited}"
    log_info "Command: claude -p \"$cmd\""
    [[ "$STOP_ON_COMPLETE" == true ]] && log_info "Will stop after current task completes"
    echo ""

    local iteration=0
    local tasks_completed=0
    local start_time
    start_time=$(date +%s)

    # On interrupt: clean up any lock files left by a killed claude session.
    # Lock files contain a timestamp; any lock present after claude exits is stale.
    cleanup_locks() {
        local lock_dir=".jdi/locks"
        if [[ -d "$lock_dir" ]]; then
            local locks
            locks=$(find "$lock_dir" -name '*.lock' 2>/dev/null || true)
            if [[ -n "$locks" ]]; then
                log_warn "Cleaning up lock files left by interrupted session..."
                rm -f "$lock_dir"/*.lock
                log_ok "Lock files removed"
            fi
        fi
    }
    trap 'echo ""; log_warn "Interrupted (Ctrl+C)"; cleanup_locks; exit 130' INT TERM

    while true; do
        iteration=$((iteration + 1))

        # Check max iterations (0 = unlimited)
        if [[ $MAX_ITERATIONS -gt 0 ]] && [[ $iteration -gt $MAX_ITERATIONS ]]; then
            log_warn "Max iterations ($MAX_ITERATIONS) reached"
            echo ""
            log_info "Summary: Stopped after $((iteration - 1)) iterations, $tasks_completed tasks completed"
            exit 2
        fi

        # Show iteration header
        echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
        log_info "Iteration $iteration${MAX_ITERATIONS:+/$MAX_ITERATIONS} (tasks completed: $tasks_completed)"
        echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

        # Execute jdi — capture exit code without triggering set -e
        local iter_start claude_exit=0
        iter_start=$(date +%s)

        if $VERBOSE; then
            claude -p "$cmd" || claude_exit=$?
        else
            claude -p "$cmd" 2>&1 | tail -20 || true
            claude_exit=${PIPESTATUS[0]}
        fi

        local iter_end
        iter_end=$(date +%s)
        local iter_duration=$((iter_end - iter_start))

        # Exit immediately if interrupted by signal (Ctrl+C = 130, SIGTERM = 143)
        if [[ $claude_exit -eq 130 ]] || [[ $claude_exit -eq 143 ]]; then
            echo ""
            log_warn "Interrupted (Ctrl+C)"
            cleanup_locks
            exit 130
        fi

        # If claude exited non-zero, it may have crashed without releasing its lock.
        # Clean up any lock files so the next iteration isn't blocked.
        if [[ $claude_exit -ne 0 ]]; then
            log_warn "claude exited with code $claude_exit (checking status file...)"
            cleanup_locks
        fi

        # Read status (may contain step name, e.g., "STEP_COMPLETE step=finalize")
        local status_raw status step_name
        status_raw=$(read_status)
        status=$(parse_status "$status_raw")
        step_name=$(parse_step_name "$status_raw")

        if [[ -n "$step_name" ]]; then
            log_info "Status: $status (step: $step_name, took ${iter_duration}s)"
        else
            log_info "Status: $status (took ${iter_duration}s)"
        fi
        echo ""

        # Handle status
        case $status in
            CONTINUE)
                log_ok "Continuing to next iteration..."
                echo ""
                ;;
            STEP_COMPLETE)
                tasks_completed=$((tasks_completed + 1))
                if [[ "$STOP_ON_COMPLETE" == true ]]; then
                    local total_time=$(($(date +%s) - start_time))
                    echo ""
                    log_ok "Task completed (step: ${step_name:-unknown})! Stopping (--stop-on-complete)."
                    log_info "Total iterations: $iteration"
                    log_info "Tasks completed: $tasks_completed"
                    log_info "Total time: ${total_time}s"
                    exit 0
                fi
                log_ok "Task completed (step: ${step_name:-unknown})! Continuing to next task..."
                echo ""
                ;;
            WORKFLOW_COMPLETE)
                local total_time=$(($(date +%s) - start_time))
                echo ""
                log_ok "All tasks completed — workflow finished!"
                log_info "Total iterations: $iteration"
                log_info "Tasks completed: $tasks_completed"
                log_info "Total time: ${total_time}s"
                exit 0
                ;;
            ABORT)
                local total_time=$(($(date +%s) - start_time))
                echo ""
                log_error "Workflow aborted!"
                log_info "Failed at iteration: $iteration"
                log_info "Tasks completed before failure: $tasks_completed"
                log_info "Total time: ${total_time}s"
                log_info "Check .jdi/reports/ for details"
                exit 1
                ;;
            HUMAN_REQUIRED)
                echo ""
                log_warn "Human step reached. Run '/jdi run --human' to continue, then restart the loop."
                exit 3
                ;;
            *)
                log_error "Unknown status: $status"
                exit 1
                ;;
        esac
    done
}

# Run main
main "$@"
