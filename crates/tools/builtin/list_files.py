# /// script
# requires-python = ">=3.14"
# dependencies = []
#
# [tool.chummy]
# name = "list_files"
# title = "List files"
# version = "1.0.0"
# ///
"""Lists a folder in the agent's workspace."""

import os

import chummy


@chummy.tool
def list_files(path: str = ".") -> str:
    """List the files and folders in a workspace folder. Folders end with /.

    path: the folder, relative to the workspace; . is the workspace itself
    """
    full = chummy.workspace_path(path)
    try:
        names = sorted(os.listdir(full), key=str.lower)
    except FileNotFoundError:
        raise chummy.ToolError(f"Folder not found: {path}")
    except NotADirectoryError:
        raise chummy.ToolError(f"{path} is a file. Use read_file to read it.")
    if not names:
        return "(empty folder)"
    out = [n + "/" if os.path.isdir(os.path.join(full, n)) else n for n in names[:500]]
    if len(names) > 500:
        out.append(f"[… and {len(names) - 500} more]")
    return "\n".join(out)
