import DOMPurify from "dompurify";
import { marked } from "marked";
import { useMemo } from "react";

export default function MarkdownPreview({ content }: { content: string }) {
  const html = useMemo(
    () => DOMPurify.sanitize(marked.parse(content, { async: false })),
    [content],
  );
  // biome-ignore lint/security/noDangerouslySetInnerHtml: repository markdown is sanitized before rendering
  return <div className="fv-md" dangerouslySetInnerHTML={{ __html: html }} />;
}
