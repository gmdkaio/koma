# sourced by the build scripts. fills the oauth env vars from the given client json when unset.
oauth_json="$1"

json_field() {
    grep -o "\"$1\": *\"[^\"]*\"" "$oauth_json" | head -1 | sed 's/.*: *"\(.*\)"/\1/'
}

if [[ -z "${KOMA_GDRIVE_CLIENT_ID:-}" && -f "$oauth_json" ]]; then
    export KOMA_GDRIVE_CLIENT_ID="$(json_field client_id)"
    export KOMA_GDRIVE_CLIENT_SECRET="$(json_field client_secret)"
fi

if [[ -n "${KOMA_GDRIVE_CLIENT_ID:-}" ]]; then
    echo "baking google drive client into the build"
else
    echo "no google drive client found, users will have to import their own json"
fi
