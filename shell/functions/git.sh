
# Return the repository root for the current directory.
_gwRepoRoot() {
    git rev-parse --show-toplevel 2>/dev/null
}

# Return the top-level directory of the current worktree.
_gwWorktreeRoot() {
    git rev-parse --show-toplevel 2>/dev/null
}

# Return the suitcase worktree config directory.
_gwConfigDir() {
    if [ -n "$XDG_CONFIG_HOME" ]; then
        echo "$XDG_CONFIG_HOME/suitcase/worktrees"
        return 0
    fi

    echo "$HOME/.config/suitcase/worktrees"
}

# Pick the first matching config file for this repository.
_gwConfigFile() {
    local repo_root="$1"
    local config_dir
    config_dir=$(_gwConfigDir)

    if [ ! -d "$config_dir" ]; then
        return 1
    fi

    local repo_name
    repo_name=$(basename "$repo_root")
    local candidate
    for candidate in \
        "$config_dir/$repo_name" \
        "$config_dir/$repo_name.conf" \
        "$config_dir/default" \
        "$config_dir/default.conf"
    do
        if [ -f "$candidate" ]; then
            echo "$candidate"
            return 0
        fi
    done

    return 1
}

# Read a configured worktree setting from the matching config file.
_gwConfigValue() {
    local repo_root="$1"
    local key="$2"
    local config_file
    config_file=$(_gwConfigFile "$repo_root") || return 1

    (
        source "$config_file" >/dev/null 2>&1 || exit 1
        case "$key" in
            remote) printf '%s\n' "${GW_REMOTE:-${remote:-}}" ;;
            base_branch) printf '%s\n' "${GW_BASE_BRANCH:-${base_branch:-}}" ;;
            worktree_dir) printf '%s\n' "${GW_WORKTREE_DIR:-${worktree_dir:-}}" ;;
            *) exit 1 ;;
        esac
    )
}

# Report where the effective defaults are coming from.
_gwValueSource() {
    local repo_root="$1"
    local key="$2"
    local env_name=

    case "$key" in
        remote) env_name="GW_REMOTE" ;;
        base_branch) env_name="GW_BASE_BRANCH" ;;
        worktree_dir) env_name="GW_WORKTREE_DIR" ;;
        *) return 1 ;;
    esac

    if [ -n "${!env_name}" ]; then
        echo "env:$env_name"
        return 0
    fi

    local config_file
    config_file=$(_gwConfigFile "$repo_root") || {
        echo "auto"
        return 0
    }

    if [ -n "$(_gwConfigValue "$repo_root" "$key" 2>/dev/null)" ]; then
        echo "config:$config_file"
        return 0
    fi

    echo "auto"
}

# Pick a default remote, preferring origin when available.
_gwPrimaryRemote() {
    local repo_root
    repo_root=$(_gwRepoRoot)

    if [ -n "$GW_REMOTE" ]; then
        echo "$GW_REMOTE"
        return 0
    fi

    if [ -n "$repo_root" ]; then
        local configured_remote
        configured_remote=$(_gwConfigValue "$repo_root" remote 2>/dev/null)
        if [ -n "$configured_remote" ]; then
            echo "$configured_remote"
            return 0
        fi
    fi

    if git remote | grep -Fxq "origin"; then
        echo "origin"
        return 0
    fi

    git remote | head -n 1
}

# Discover the remote default branch without assuming main/master.
_gwDefaultBranch() {
    local repo_root
    repo_root=$(_gwRepoRoot)

    if [ -n "$GW_BASE_BRANCH" ]; then
        echo "$GW_BASE_BRANCH"
        return 0
    fi

    if [ -n "$repo_root" ]; then
        local configured_base_branch
        configured_base_branch=$(_gwConfigValue "$repo_root" base_branch 2>/dev/null)
        if [ -n "$configured_base_branch" ]; then
            echo "$configured_base_branch"
            return 0
        fi
    fi

    local remote="$1"
    local default_branch

    default_branch=$(git symbolic-ref --quiet --short "refs/remotes/$remote/HEAD" 2>/dev/null)
    default_branch=${default_branch#"$remote"/}
    if [ -n "$default_branch" ]; then
        echo "$default_branch"
        return 0
    fi

    default_branch=$(git ls-remote --symref "$remote" HEAD 2>/dev/null | awk '/^ref:/ {sub("refs/heads/", "", $2); print $2; exit}')
    if [ -n "$default_branch" ]; then
        echo "$default_branch"
        return 0
    fi

    return 1
}

# Store worktrees inside the repository by default.
_gwWorktreeBase() {
    local repo_root="$1"

    if [ -n "$GW_WORKTREE_DIR" ]; then
        case "$GW_WORKTREE_DIR" in
            /*) echo "$GW_WORKTREE_DIR" ;;
            *) echo "$repo_root/$GW_WORKTREE_DIR" ;;
        esac
        return 0
    fi

    local configured_worktree_dir
    configured_worktree_dir=$(_gwConfigValue "$repo_root" worktree_dir 2>/dev/null)
    if [ -n "$configured_worktree_dir" ]; then
        case "$configured_worktree_dir" in
            /*) echo "$configured_worktree_dir" ;;
            *) echo "$repo_root/$configured_worktree_dir" ;;
        esac
        return 0
    fi

    echo "$repo_root/.worktrees"
}

# Avoid showing the repo-owned worktree directory as untracked content.
_gwEnsureExcluded() {
    local repo_root="$1"
    local worktree_base="$2"
    local git_dir
    git_dir=$(git rev-parse --git-dir 2>/dev/null) || return 1

    case "$worktree_base" in
        "$repo_root"/*)
            local relative_path=${worktree_base#"$repo_root"/}
            local exclude_file="$git_dir/info/exclude"
            local exclude_rule="/$relative_path/"
            mkdir -p "$(dirname "$exclude_file")" || return 1
            touch "$exclude_file" || return 1
            grep -Fqx "$exclude_rule" "$exclude_file" || printf '%s\n' "$exclude_rule" >> "$exclude_file"
            ;;
    esac
}

# Git worktree function to create new branch from main or checkout existing remote
gwA() {
    if [ $# -ne 1 ]; then
        echo "Usage: gwA <branch-name>"
        echo "Creates worktree at the configured worktree base or <repo>/.worktrees/<branch-name> by default"
        echo "- If remote branch exists: checks out existing branch"
        echo "- If remote branch doesn't exist: creates new branch from the remote default branch"
        return 1
    fi

    local branch_name="$1"
    local repo_root
    repo_root=$(_gwRepoRoot) || {
        echo "Error: Not in a git repository"
        return 1
    }
    local remote
    remote=$(_gwPrimaryRemote)
    if [ -z "$remote" ]; then
        echo "Error: No git remotes configured"
        return 1
    fi
    local worktree_base
    worktree_base=$(_gwWorktreeBase "$repo_root")
    local worktree_path="$worktree_base/$branch_name"

    mkdir -p "$worktree_base" || {
        echo "Failed to create worktree base directory: $worktree_base"
        return 1
    }
    _gwEnsureExcluded "$repo_root" "$worktree_base" || {
        echo "Failed to update local exclude rules for: $worktree_base"
        return 1
    }

    git fetch "$remote" || {
        echo "Failed to fetch remote: $remote"
        return 1
    }

    # Check if remote branch exists
    if git show-ref --verify --quiet "refs/remotes/$remote/$branch_name"; then
        echo "Remote branch $remote/$branch_name exists, checking out existing branch"
        if git show-ref --verify --quiet "refs/heads/$branch_name"; then
            # Local branch already exists, just create worktree from it
            git worktree add "$worktree_path" "$branch_name"
        else
            git worktree add --track -b "$branch_name" "$worktree_path" "$remote/$branch_name"
        fi
    else
        local base_branch
        base_branch=$(_gwDefaultBranch "$remote") || {
            echo "Failed to determine the default branch for remote: $remote"
            return 1
        }
        echo "Remote branch $remote/$branch_name doesn't exist, creating new branch from $remote/$base_branch"
        if git show-ref --verify --quiet "refs/heads/$branch_name"; then
            git worktree add "$worktree_path" "$branch_name"
        else
            git worktree add -b "$branch_name" "$worktree_path" "$remote/$base_branch"
        fi
    fi

    if [ $? -ne 0 ]; then
        echo "Failed to create worktree"
        return 1
    fi

    # Change to the newly created worktree
    cd "$worktree_path"
}

# Report effective worktree defaults and current worktree state.
gwStat() {
    local repo_root
    repo_root=$(_gwRepoRoot) || {
        echo "Error: Not in a git repository"
        return 1
    }

    local config_file="none"
    if [ -n "$(_gwConfigFile "$repo_root" 2>/dev/null)" ]; then
        config_file=$(_gwConfigFile "$repo_root")
    fi

    local remote
    remote=$(_gwPrimaryRemote)
    local base_branch
    base_branch=$(_gwDefaultBranch "$remote" 2>/dev/null)
    local worktree_base
    worktree_base=$(_gwWorktreeBase "$repo_root")
    local current_root
    current_root=$(_gwWorktreeRoot)

    echo "Repository: $repo_root"
    echo "Current root: $current_root"
    echo "Config file: $config_file"
    echo "Remote: ${remote:-<none>} [$(_gwValueSource "$repo_root" remote)]"
    echo "Base branch: ${base_branch:-<unknown>} [$(_gwValueSource "$repo_root" base_branch)]"
    echo "Worktree base: $worktree_base [$(_gwValueSource "$repo_root" worktree_dir)]"
    echo ""
    echo "Worktrees:"

    local common_dir
    common_dir=$(git rev-parse --git-common-dir 2>/dev/null)
    local path_to_wt=
    local commit=
    local branch=
    local bare=0
    while IFS= read -r line || [ -n "$line" ]; do
        if [ -z "$line" ]; then
            if [ -n "$path_to_wt" ]; then
                local label status dirty marker branch_label
                label="$path_to_wt"
                status="ok"
                dirty="clean"
                marker=" "
                branch_label="$branch"

                if [ "$bare" -eq 1 ]; then
                    status="bare"
                    branch_label="-"
                elif [ "$branch" = "detached" ]; then
                    status="detached@$commit"
                    branch_label="detached"
                else
                    if [ -n "$remote" ] && ! git ls-remote --exit-code --heads "$remote" "$branch" &>/dev/null; then
                        status="stale"
                    fi
                    if [ -d "$path_to_wt/.git" ] || [ -f "$path_to_wt/.git" ]; then
                        if ! git -C "$path_to_wt" diff --quiet --ignore-submodules -- 2>/dev/null || ! git -C "$path_to_wt" diff --cached --quiet --ignore-submodules -- 2>/dev/null; then
                            dirty="dirty"
                        fi
                    else
                        dirty="missing"
                    fi
                fi

                [ "$path_to_wt" = "$current_root" ] && marker="*"
                printf "%s %s | branch=%s | %s | %s\n" "$marker" "$label" "${branch_label:-?}" "$status" "$dirty"
            fi

            path_to_wt=
            commit=
            branch=
            bare=0
            continue
        fi

        case "$line" in
            worktree\ *) path_to_wt=${line#worktree } ;;
            HEAD\ *) commit=${line#HEAD } ;;
            branch\ refs/heads/*) branch=${line#branch refs/heads/} ;;
            detached) branch="detached" ;;
            bare) bare=1 ;;
        esac
    done < <(git --git-dir="$common_dir" worktree list --porcelain; printf '\n')
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

    local git_root
    git_root=$(_gwRepoRoot)
    echo "gwD: Git repository root: $git_root"
    local worktree_root
    worktree_root=$(_gwWorktreeRoot)
    echo "gwD: Worktree root: $worktree_root"
    local branch_name=$(git branch --show-current)
    echo "gwD: Current branch: $branch_name"

    # Check for uncommitted changes
    if ! git diff --quiet || ! git diff --cached --quiet; then
        echo "gwD: WARNING - You have uncommitted changes in this worktree:"
        git status --porcelain
        echo ""
    fi

    echo "gwD: About to remove worktree: $worktree_root"
    echo "gwD: This will permanently delete the worktree directory and all its contents"

    # Move to git repository root for the removal operation
    echo "gwD: Changing to git repository root: $git_root"
    cd "$git_root"

    # Remove the worktree (git worktree remove has built-in confirmation prompts)
    echo "gwD: Executing: git worktree remove '$worktree_root'"
    git worktree remove "$worktree_root"

    if [ $? -eq 0 ]; then
        echo "gwD: Successfully removed worktree: $worktree_root"
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
    local remote
    remote=$(_gwPrimaryRemote)
    if [ -z "$remote" ]; then
        echo "Error: No git remotes configured"
        return 1
    fi

    local path_to_wt=
    local commit=
    local branch=
    while IFS= read -r line || [ -n "$line" ]; do
        if [ -z "$line" ]; then
            if [ "$commit" = "bare" ]; then
                path_to_wt=
                commit=
                branch=
                continue
            fi
            if [ "$branch" = "detached" ]; then
                echo "  $path_to_wt (detached at $commit)"
            elif [ -n "$branch" ] && ! git ls-remote --exit-code --heads "$remote" "$branch" &>/dev/null; then
                stale_paths+=("$path_to_wt")
                stale_branches+=("$branch")
            fi
            path_to_wt=
            commit=
            branch=
            continue
        fi

        case "$line" in
            worktree\ *) path_to_wt=${line#worktree } ;;
            HEAD\ *) commit=${line#HEAD } ;;
            branch\ refs/heads/*) branch=${line#branch refs/heads/} ;;
            detached) branch="detached" ;;
        esac
    done < <(git worktree list --porcelain; printf '\n')

    return 0
}

# Check for stale worktrees (where remote branch has been deleted)
gwCheck() {
    echo "Checking for stale worktrees (remote branches deleted)..."
    echo "=================================================="

    _gwStaleWorktrees || return 1

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

    _gwStaleWorktrees || return 1

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
