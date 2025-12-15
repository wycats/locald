import os
import re

def fix_file(filepath):
    try:
        with open(filepath, 'r') as f:
            lines = f.readlines()

        new_lines = []
        changed = False
        in_frontmatter = False
        
        for i, line in enumerate(lines):
            if line.strip() == '---':
                if i == 0:
                    in_frontmatter = True
                elif in_frontmatter:
                    in_frontmatter = False
                new_lines.append(line)
                continue
            
            if in_frontmatter and line.startswith('title:'):
                # Check if it needs quotes
                value = line[6:].strip()
                if ':' in value and not (value.startswith('"') and value.endswith('"')):
                    new_lines.append(f'title: "{value}"\n')
                    changed = True
                else:
                    new_lines.append(line)
            else:
                new_lines.append(line)

        if changed:
            with open(filepath, 'w') as f:
                f.writelines(new_lines)
            print(f"Fixed quotes in {filepath}")

    except Exception as e:
        print(f"Error processing {filepath}: {e}")

def walk_dir(root_dir):
    for root, dirs, files in os.walk(root_dir):
        for file in files:
            if file.endswith('.md') or file.endswith('.mdx'):
                fix_file(os.path.join(root, file))

walk_dir('src/content/docs')
