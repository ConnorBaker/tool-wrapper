# Install a prebuilt shim at PROGRAM and keep the executable beside it.
installAdjacentWrapper() {
    local program=$1 wrapper=$2
    [[ $program == */* ]] || program=./$program
    local directory=${program%/*} name=${program##*/}
    local vendor="$directory/.$name-wrapped" staged

    if [[ ! -f "$program" || ! -x "$program" || ! -f "$wrapper" || ! -x "$wrapper" ]]; then
        printf 'installAdjacentWrapper: expected executables at %s and %s\n' "$program" "$wrapper" >&2
        return 1
    fi
    if [[ -e "$vendor" || -L "$vendor" ]]; then
        printf 'installAdjacentWrapper: %s already exists\n' "$vendor" >&2
        return 1
    fi

    staged=$(mktemp -- "$directory/.tool-wrapper.XXXXXXXX") || return
    if ! install -m 0755 -- "$wrapper" "$staged" || ! mv -T --update=none-fail -- "$program" "$vendor"; then
        rm -f -- "$staged"
        return 1
    fi
    if ! mv -T --update=none-fail -- "$staged" "$program"; then
        rm -f -- "$staged"
        mv -T --update=none-fail -- "$vendor" "$program"
        return 1
    fi
}
