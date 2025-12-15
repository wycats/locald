#!/bin/bash
set -e

# Get the directory of the script
SCRIPT_DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" && pwd )"
LOCALD_DOCS_DIR="$(dirname "$SCRIPT_DIR")"
REPO_ROOT="$(dirname "$LOCALD_DOCS_DIR")"

# Define source and destination
SRC_DESIGN="$REPO_ROOT/docs/design"
DEST_CONCEPTS="$LOCALD_DOCS_DIR/src/content/docs/concepts"
DEST_INTERNALS="$LOCALD_DOCS_DIR/src/content/docs/internals"

# Ensure destinations exist
mkdir -p "$DEST_CONCEPTS"
mkdir -p "$DEST_INTERNALS/axioms"

# Function to copy with frontmatter injection
copy_with_frontmatter() {
    local src_file="$1"
    local dest_file="$2"
    local title_override="$3"

    if [ ! -f "$src_file" ]; then
        echo "Warning: Source file not found: $src_file"
        return
    fi

    mkdir -p "$(dirname "$dest_file")"

    # Extract title from the first line starting with #
    local title
    if [ -n "$title_override" ]; then
        title="$title_override"
    else
        title=$(grep -m 1 "^# " "$src_file" | sed 's/^# //')
    fi

    echo "Syncing $src_file -> $dest_file (Title: $title)"

    # Write frontmatter
    echo "---" > "$dest_file"
    echo "title: \"$title\"" >> "$dest_file"
    echo "---" >> "$dest_file"

    # Append content, removing the first line starting with #
    awk '/^# / && !found { found=1; next } { print }' "$src_file" >> "$dest_file"
}

# Sync Concepts
CONCEPTS=(
    "vision.md"
    "generative-design.md"
    "modes.md"
    "personas.md"
    "user-interaction-modes.md"
    "workflow-axioms.md"
)

for file in "${CONCEPTS[@]}"; do
    copy_with_frontmatter "$SRC_DESIGN/$file" "$DEST_CONCEPTS/$file"
done

# Sync Internals (Axioms Index)
copy_with_frontmatter "$SRC_DESIGN/axioms.md" "$DEST_INTERNALS/axioms.md"

# Sync Internals (Axioms Subfiles)
if [ -d "$SRC_DESIGN/axioms" ]; then
    # Avoid stale generated pages if the source tree changes.
    rm -rf "$DEST_INTERNALS/axioms"
    mkdir -p "$DEST_INTERNALS/axioms"

    find "$SRC_DESIGN/axioms" -name "*.md" | while read src_file; do
        rel_path="${src_file#"$SRC_DESIGN/axioms/"}"
        copy_with_frontmatter "$src_file" "$DEST_INTERNALS/axioms/$rel_path"
    done
fi

echo "Design docs synced to $DEST_CONCEPTS and $DEST_INTERNALS"
