import { useEffect, useState } from "react";
import { subscribe, type Politeness } from "./announcer";

/**
 * The two live regions every announcement goes through. Text is cleared
 * before each new message so repeating the same message is still spoken.
 */
export function LiveRegions() {
  const [messages, setMessages] = useState<Record<Politeness, string>>({ polite: "", assertive: "" });

  useEffect(
    () =>
      subscribe((message, politeness) => {
        setMessages((current) => ({ ...current, [politeness]: "" }));
        requestAnimationFrame(() => setMessages((current) => ({ ...current, [politeness]: message })));
      }),
    [],
  );

  return (
    <>
      <div className="visually-hidden" role="status" aria-live="polite" aria-atomic="true">
        {messages.polite}
      </div>
      <div className="visually-hidden" role="alert" aria-live="assertive" aria-atomic="true">
        {messages.assertive}
      </div>
    </>
  );
}
