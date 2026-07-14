
# Git worktree function to create new branch from main or checkout existing remote
gwA() {
    if [ $# -ne 1 ]; then
        echo "Usage: gwA <branch-name>"
        echo "Creates worktree at ../worktrees/<branch-name>"
        echo "- If remote branch exists: checks out existing branch"
        echo "- If remote branch doesn't exist: creates new branch from main"
        return 1
    fi

    local branch_name="$1"
    local worktree_path="../worktrees/$branch_name"

    git fetch origin

    # Check if remote branch exists
    if git show-ref --verify --quiet "refs/remotes/origin/$branch_name"; then
        echo "Remote branch origin/$branch_name exists, checking out existing branch"
        if git show-ref --verify --quiet "refs/heads/$branch_name"; then
            # Local branch already exists, just create worktree from it
            git worktree add "$worktree_path" "$branch_name"
        else
            git worktree add -b "$branch_name" "$worktree_path" "origin/$branch_name"
        fi
    else
        echo "Remote branch origin/$branch_name doesn't exist, creating new branch from main"
        if git show-ref --verify --quiet "refs/heads/$branch_name"; then
            git worktree add "$worktree_path" "$branch_name"
        else
            git worktree add -b "$branch_name" "$worktree_path" origin/main
        fi
    fi

    if [ $? -ne 0 ]; then
        echo "Failed to create worktree"
        return 1
    fi

    # Change to the newly created worktree
    cd "$worktree_path"
}

# Git worktree function to delete current directory as worktree
gwD() {
    local current_dir=$(pwd)

    echo "gwD: Analyzing current directory: $current_dir"

    # Check if we're in a git worktree (not the main repo)
    if ! git rev-parse --is-inside-work-tree &>/dev/null; then
        echo "Error: Not in a git repository"
        return 1
    fi
    echo "gwD: Confirmed we're in a git repository"

    # Check if this is a worktree (not the main repository)
    if [ "$(git rev-parse --git-common-dir)" = "$(git rev-parse --git-dir)" ]; then
        echo "Error: You're in the main repository, not a worktree"
        echo "gwD only works from within a worktree directory"
        return 1
    fi
    echo "gwD: Confirmed we're in a worktree (not main repository)"

    local git_root=$(git rev-parse --git-common-dir | sed 's/\.git$//')
    echo "gwD: Git repository root: $git_root"
    local branch_name=$(git branch --show-current)
    echo "gwD: Current branch: $branch_name"

    # Check for uncommitted changes
    if ! git diff --quiet || ! git diff --cached --quiet; then
        echo "gwD: WARNING - You have uncommitted changes in this worktree:"
        git status --porcelain
        echo ""
    fi

    echo "gwD: About to remove worktree: $current_dir"
    echo "gwD: This will permanently delete the worktree directory and all its contents"

    # Move to git repository root for the removal operation
    echo "gwD: Changing to git repository root: $git_root"
    cd "$git_root"

    # Remove the worktree (git worktree remove has built-in confirmation prompts)
    echo "gwD: Executing: git worktree remove '$current_dir'"
    git worktree remove "$current_dir"

    if [ $? -eq 0 ]; then
        echo "gwD: Successfully removed worktree: $current_dir"
        # Delete the local branch now that the worktree is gone
        if [ -n "$branch_name" ]; then
            echo "gwD: Deleting local branch: $branch_name"
            git branch -D "$branch_name"
        fi
    else
        echo "gwD: Failed to remove worktree: $current_dir"
        # Move back to original directory if removal failed
        cd "$current_dir"
        return 1
    fi
}

# Helper: collect stale worktrees into $stale_paths and $stale_branches arrays
_gwStaleWorktrees() {
    stale_paths=()
    stale_branches=()

    readarray -t blocks < <(git worktree list --porcelain | awk 'BEGIN{RS="\n\n"} NF {gsub(/\n/, "\t"); print $0}')
    for block in "${blocks[@]}"; do
        readarray -t fields < <(echo "$block" | tr '\t' '\n')
        local path_to_wt=${fields[0]#worktree }
        local commit=${fields[1]#HEAD }
        if [ "$commit" = "bare" ]; then
            continue
        fi
        local branch=${fields[2]#branch refs/heads/}
        if [ "$branch" = "detached" ]; then
            echo "  $path_to_wt (detached at $commit)"
            continue
        fi

        if ! git ls-remote --exit-code --heads origin "$branch" &>/dev/null; then
            stale_paths+=("$path_to_wt")
            stale_branches+=("$branch")
        fi
    done
}

# Check for stale worktrees (where remote branch has been deleted)
gwCheck() {
    echo "Checking for stale worktrees (remote branches deleted)..."
    echo "=================================================="

    _gwStaleWorktrees

    if [ ${#stale_paths[@]} -eq 0 ]; then
        echo "No stale worktrees found."
        return 0
    fi

    for i in "${!stale_paths[@]}"; do
        echo "${stale_paths[$i]} (${stale_branches[$i]} does not exist on origin)"
    done
}

# Remove stale worktrees whose remote branch has been deleted
gwPrune() {
    echo "Scanning for stale worktrees..."
    echo "=================================================="

    _gwStaleWorktrees

    if [ ${#stale_paths[@]} -eq 0 ]; then
        echo "No stale worktrees to prune."
        return 0
    fi

    echo ""
    echo "The following worktrees will be removed:"
    for i in "${!stale_paths[@]}"; do
        echo "  ${stale_paths[$i]}  (branch: ${stale_branches[$i]})"
    done
    echo ""

    read -p "Remove all ${#stale_paths[@]} stale worktree(s)? [y/N] " confirm
    if [[ "$confirm" != [yY] ]]; then
        echo "Aborted."
        return 0
    fi

    local failed=0
    for i in "${!stale_paths[@]}"; do
        echo "Removing ${stale_paths[$i]}..."
        if git worktree remove "${stale_paths[$i]}" 2>/dev/null; then
            echo "  Removed."
        elif git worktree remove --force "${stale_paths[$i]}" 2>/dev/null; then
            echo "  Force-removed (had modifications)."
        else
            echo "  FAILED to remove ${stale_paths[$i]}"
            ((failed++))
        fi
    done

    git worktree prune

    # Delete local branches for successfully removed worktrees
    for i in "${!stale_branches[@]}"; do
        if ! git worktree list --porcelain | grep -q "branch refs/heads/${stale_branches[$i]}$"; then
            echo "Deleting local branch: ${stale_branches[$i]}"
            git branch -D "${stale_branches[$i]}" 2>/dev/null
        fi
    done

    if [ $failed -gt 0 ]; then
        echo "$failed worktree(s) could not be removed."
        return 1
    fi
    echo "Done. All stale worktrees and branches removed."
}
