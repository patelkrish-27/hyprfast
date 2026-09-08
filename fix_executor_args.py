import re

with open("src/browser_runtime/executor.rs", "r") as f:
    content = f.read()

content = content.replace("_session_id: Option<&str>,", "session_id: Option<&str>,")

with open("src/browser_runtime/executor.rs", "w") as f:
    f.write(content)
