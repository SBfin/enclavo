export async function readChatStream(response: Response, onDelta: (text: string) => void): Promise<number | null> {
  if (!response.body) throw new Error("The provider returned an empty stream.");
  const reader = response.body.getReader();
  const decoder = new TextDecoder();
  let buffer = "";
  let data: string[] = [];
  let done = false;
  let cost: number | null = null;
  const event = () => {
    if (!data.length) return;
    const text = data.join("\n");
    data = [];
    if (text === "[DONE]") { done = true; return; }
    const chunk = JSON.parse(text);
    if (chunk.error) throw new Error(chunk.error.message || "The provider stream failed.");
    const delta = chunk.choices?.[0]?.delta?.content;
    if (typeof delta === "string") onDelta(delta);
  };
  const line = (text: string) => {
    if (!text) { event(); return; }
    if (text.startsWith("data:")) data.push(text.slice(5).replace(/^ /, ""));
    if (text.startsWith(": enclavo-cost-micro-usd ")) {
      const value = Number(text.slice(": enclavo-cost-micro-usd ".length));
      if (Number.isSafeInteger(value) && value >= 0) cost = value;
    }
  };
  try {
    while (!done) {
      const chunk = await reader.read();
      buffer += decoder.decode(chunk.value, { stream: !chunk.done });
      let boundary: number;
      while ((boundary = buffer.indexOf("\n")) >= 0) {
        line(buffer.slice(0, boundary).replace(/\r$/, ""));
        buffer = buffer.slice(boundary + 1);
      }
      if (chunk.done) {
        if (buffer) line(buffer.replace(/\r$/, ""));
        event();
        break;
      }
    }
    if (!done) throw new Error("The provider stream ended before completion. The displayed answer may be incomplete.");
    return cost;
  } finally {
    await reader.cancel().catch(() => {});
    reader.releaseLock();
  }
}
