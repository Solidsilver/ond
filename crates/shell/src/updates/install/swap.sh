# Paths arrive as arguments, never interpolated shell source. The replacement
# and backup share the target's filesystem so the final swap is a rename.
trap '' HUP
pid=$1 target=$2 replacement=$3 backup=$4 stage=$5 neighbor=$6 mode=$7 prefix=$8 launch=$9
while [ "$pid" != 0 ] && kill -0 "$pid" 2>/dev/null; do sleep 1; done
reopen() {
  case "$launch" in
    macos) /usr/bin/open "$target" ;;
    binary) "$target" </dev/null >/dev/null 2>&1 & ;;
  esac
}
fail() {
  printf '%s\n' "$1" > "$(dirname "$stage")/last-install-error"
  rm -rf "$stage" "$neighbor"
  reopen
  exit 1
}
mv "$target" "$backup" || fail 'Could not move the installed app aside.'
if [ "$mode" = layout ]; then
  sh "$replacement/install.sh" "$prefix"
  result=$?
else
  mv "$replacement" "$target"
  result=$?
fi
if [ "$result" != 0 ]; then
  rm -rf "$target"
  if ! mv "$backup" "$target"; then
    # Keep the backup when even rollback cannot write the destination.
    touch "$stage/recovery"
    printf '%s\n' "Update failed. Restore the previous app from $backup." > "$(dirname "$stage")/last-install-error"
    exit 1
  fi
  fail 'The update failed; the previous app was restored.'
fi
rm -rf "$backup" "$stage" "$neighbor"
reopen
