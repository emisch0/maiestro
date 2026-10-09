// Put text on the system clipboard from a click handler. The async Clipboard
// API is the normal path (both webviews allow it on a user gesture); if it is
// missing or refuses, fall back to selecting a hidden textarea and running the
// legacy copy command. Resolves to whether either way worked.
export async function copyText(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    const area = document.createElement("textarea");
    area.value = text;
    area.setAttribute("readonly", "");
    area.style.position = "fixed";
    area.style.opacity = "0";
    document.body.appendChild(area);
    area.select();
    try {
      return document.execCommand("copy");
    } catch {
      return false;
    } finally {
      area.remove();
    }
  }
}
