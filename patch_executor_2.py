import re

with open("src/browser_runtime/executor.rs", "r") as f:
    content = f.read()

content = content.replace("if let Some(ref r) = resolved_ref {", "if let Some(ResolvedTarget::Element(ref r)) = resolved_ref {")

with open("src/browser_runtime/executor.rs", "w") as f:
    f.write(content)
