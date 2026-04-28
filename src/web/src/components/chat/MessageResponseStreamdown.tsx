"use client";

import { cjk } from "@streamdown/cjk";
import { code } from "@streamdown/code";
import { Streamdown } from "streamdown";

import type { MessageResponseProps } from "./MessageResponse";

/** Heavy markdown/code renderer loaded only when assistant output is shown. */
export function MessageResponseStreamdown({
  content,
  isStreaming = false,
  className,
}: MessageResponseProps) {
  return (
    <Streamdown
      className={[
        "prose prose-sm dark:prose-invert max-w-none text-sm",
        "[&>*:first-child]:mt-0 [&>*:last-child]:mb-0",
        className ?? "",
      ]
        .filter(Boolean)
        .join(" ")}
      plugins={{ cjk, code }}
      shikiTheme={["github-light", "github-dark"]}
      isAnimating={isStreaming}
      parseIncompleteMarkdown={true}
    >
      {content}
    </Streamdown>
  );
}
