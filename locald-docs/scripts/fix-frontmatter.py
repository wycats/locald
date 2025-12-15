import os
import re

def fix_file(filepath):
    try:
        with open(filepath, 'r') as f:
            content = f.read()

        if content.startswith('---'):
            return # Already has frontmatter

        # Find first H1
        match = re.search(r'^#\s+(.+)$', content, re.MULTILINE)
        if match:
            title = match.group(1).strip()
            # Remove the H1 line
            new_content = content[:match.start()] + content[match.end():]
            # Add frontmatter
            new_content = f"---\ntitle: {title}\n---\n\n" + new_content.lstrip()
            
            with open(filepath, 'w') as f:
                f.write(new_content)
            print(f"Fixed {filepath}")
        else:
            print(f"Skipping {filepath} (no H1 found)")
    except Exception as e:
        print(f"Error processing {filepath}: {e}")

def walk_dir(root_dir):
    print(f"Scanning {root_dir}...")
    if not os.path.exists(root_dir):
        print(f"Directory not found: {root_dir}")
        return
    for root, dirs, files in os.walk(root_dir):
        for file in files:
            if file.endswith('.md') or file.endswith('.mdx'):
                fix_file(os.path.join(root, file))

walk_dir('src/content/docs')
