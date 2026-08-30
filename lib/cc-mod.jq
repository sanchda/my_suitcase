# jq library for cc-mod: reversible merges into ~/.claude/settings.json.
#
# A mod contributes a settings fragment. Applying it produces a list of ops that
# is written to a receipt, and disabling replays those ops backwards -- so a
# hooks array keeps every entry other mods (or you) put there, and a key that
# existed before is restored to its old value rather than deleted.
#
# op = {op: "set",    path: [...], value: v, had: bool, prev: v}
#      {op: "append", path: [...], value: element}

# Replace {{NAME}} in every string of the input using $vars ({NAME: "value"}).
def subst($vars):
  walk(
    if type == "string" then
      reduce ($vars | to_entries[]) as $v (.; gsub("\\{\\{" + $v.key + "\\}\\}"; $v.value))
    else . end
  );

# Ops that would merge $frag into the input settings.
#
# Objects recurse -- including where the settings have nothing yet, since a null
# `$cur` indexes to null all the way down. Arrays append their elements; anything
# else is a set. Recording leaves rather than one coarse "set the whole subtree"
# op is what keeps mods independent: if this mod created `hooks` and another later
# appended to `hooks.Stop`, disabling this one takes only its own leaf.
#
# Adoption: a value already equal to what the mod declares is recorded as the
# mod's own (had: false; appends are recorded even when the element is present),
# so enabling over matching config takes ownership and disabling removes it. A
# value that differs is kept in `prev` and restored on disable.
def ops($frag):
  def go($path; $cur; $f):
    if ($f | type) == "object" and (($cur | type) == "object" or $cur == null) then
      reduce ($f | keys_unsorted[]) as $k ([]; . + go($path + [$k]; $cur[$k]; $f[$k]))
    elif ($f | type) == "array" then
      [ $f[] | {op: "append", path: $path, value: .} ]
    else
      (($cur != null) and ($cur != $f)) as $had
      | [ {op: "set", path: $path, value: $f, had: $had,
           prev: (if $had then $cur else null end)} ]
    end;
  go([]; .; $frag);

def apply_ops($ops):
  reduce $ops[] as $o (.;
    if $o.op == "set" then
      setpath($o.path; $o.value)
    elif $o.op == "append" then
      (getpath($o.path) // []) as $arr
      | if ($arr | type) != "array" then
          error("\($o.path | join(".")) already exists and is not a list")
        elif any($arr[]; . == $o.value) then .
        else setpath($o.path; $arr + [$o.value])
        end
    else error("unknown op \($o.op)")
    end
  );

# Delete containers we emptied. Only prefixes of the ops' own paths are eligible,
# so nothing outside the mod's footprint is touched.
def prune($ops):
  ( [ $ops[] | .path as $p | range(1; ($p | length) + 1) | $p[0:.] ]
    | unique | sort_by(-length) ) as $paths
  | reduce $paths[] as $p (.;
      (getpath($p)) as $v
      | if ($v == {} or $v == []) then delpaths([$p]) else . end);

def unapply_ops($ops):
  reduce ($ops | reverse | .[]) as $o (.;
    if $o.op == "append" then
      (getpath($o.path) // []) as $arr
      | if ($arr | type) != "array" then .
        else
          ([ range(0; $arr | length) | select($arr[.] == $o.value) ] | first) as $i
          | if $i == null then . else setpath($o.path; $arr[0:$i] + $arr[$i + 1:]) end
        end
    elif $o.op == "set" then
      if $o.had then setpath($o.path; $o.prev) else delpaths([$o.path]) end
    else .
    end
  )
  | prune($ops);

# Ops from a receipt that the live settings no longer reflect (drift).
def missing_ops($ops):
  . as $s
  | [ $ops[]
      | . as $o
      | select(
          if $o.op == "append" then
            (($s | getpath($o.path)) // []) as $arr
            | ($arr | type) != "array"
              or ([ $arr[] | select(. == $o.value) ] | length) == 0
          else
            ($s | getpath($o.path)) != $o.value
          end
        )
    ];

# What an op means, minus the had/prev bookkeeping. Two op lists with the same
# intent describe the same end state, so `ensure` can tell "already current" from
# "needs re-applying" without churning the file.
def op_intent($ops): $ops | map({op, path, value});

# One-line descriptions of an op, for terminal output.
def op_short:
  (.value | tostring) | if length > 68 then .[0:68] + "..." else . end;

def op_target:
  if .op == "append" then "\(.path | join("."))[] " + op_short
  else "\(.path | join(".")) = " + op_short
  end;

# "+" adds, "~" replaces a value that will be restored on disable.
def op_label:
  (if .op == "set" and .had then "~" else "+" end) + " " + op_target;
