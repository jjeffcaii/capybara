#!/bin/bash
set -euo pipefail

if [ "$(uname -s)" != "Linux" ]; then
    echo "Please use the GitHub Action."
    exit 1
fi

SCRIPT_DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" && pwd )"
cd $SCRIPT_DIR/..

NEW_VERSION="${1}"

echo "Bumping version: ${NEW_VERSION}"

for i in $(git ls-files '*Cargo.toml')
do
sed -i "s/^version\s*=\s*\".*/version = \"${NEW_VERSION}\"/g" $i
sed -i "s/^capybara-etc\s*=.*/capybara-etc = \"${NEW_VERSION}\"/g" $i
sed -i "s/^capybara-util\s*=.*/capybara-util = \"${NEW_VERSION}\"/g" $i
sed -i "s/^capybara-core\s*=.*/capybara-core = \"${NEW_VERSION}\"/g" $i
done

# cargo metadata --format-version 1 > /dev/null # update `Cargo.lock`
