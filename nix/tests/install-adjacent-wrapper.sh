# Run with the setup hook already loaded.
wrapper=$1
vendor_executable=$2
install_dir="$TMPDIR/"$'path with spaces\n'
mkdir -p "$install_dir"

(
    cd "$install_dir"
    program=$'probe\n'
    ln -s "$vendor_executable" "$program"
    installAdjacentWrapper "$program" "$wrapper"
    script='IFS= read -r line && printf "%s\n" "$line"'
    test "$(printf '%s\n' 'passed through' | "./$program" -c "$script")" = 'passed through'
    if installAdjacentWrapper "$program" "$wrapper"; then
        echo 'Expected a second installation to fail' >&2
        exit 1
    fi
    test -L ".$program-wrapped"
)

(
    program="$install_dir/collision"
    ln -s "$vendor_executable" "$program"
    mv() {
        # Create the hidden target after the installer's initial check.
        if [[ $4 == "$program" ]]; then
            ln -s "$vendor_executable" "$5"
        fi
        command mv "$@"
    }
    if installAdjacentWrapper "$program" "$wrapper"; then
        echo 'Expected a concurrent collision to fail' >&2
        exit 1
    fi
    test -L "$program"
    test -L "$install_dir/.collision-wrapped"
)

(
    program="$install_dir/rollback"
    ln -s "$vendor_executable" "$program"
    mv() {
        if [[ $4 == "$install_dir"/.tool-wrapper.* ]]; then
            return 1
        fi
        command mv "$@"
    }
    if installAdjacentWrapper "$program" "$wrapper"; then
        echo 'Expected the injected rename failure' >&2
        exit 1
    fi
    test -L "$program"
    test ! -L "$install_dir/.rollback-wrapped"
)

for staged in "$install_dir"/.tool-wrapper.*; do
    if [[ -e $staged || -L $staged ]]; then
        echo 'Found an abandoned staged wrapper' >&2
        exit 1
    fi
done
