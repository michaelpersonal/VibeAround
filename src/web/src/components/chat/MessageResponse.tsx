"use client";

import * as React from "react";

export type MessageResponseProps = {
  content: string;
  isStreaming?: boolean;
  className?: string;
};

const LazyMessageResponse = React.lazy(() =>
  import("./MessageResponseStreamdown").then((module) => ({
    default: module.MessageResponseStreamdown,
  }))
);

function PlainTextFallback({ content, className }: MessageResponseProps) {
  return (
    <p className={`whitespace-pre-wrap text-sm leading-7 ${className ?? ""}`}>
      {content}
    </p>
  );
}

/** Renders assistant messages while keeping the markdown/code renderer out of the initial chunk. */
export const MessageResponse = React.memo(
  (props: MessageResponseProps) => (
    <React.Suspense fallback={<PlainTextFallback {...props} />}>
      <LazyMessageResponse {...props} />
    </React.Suspense>
  ),
  (prev, next) =>
    prev.content === next.content &&
    prev.isStreaming === next.isStreaming &&
    prev.className === next.className
);
MessageResponse.displayName = "MessageResponse";
