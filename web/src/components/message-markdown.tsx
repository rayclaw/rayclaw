import React from 'react'
import ReactMarkdown from 'react-markdown'
import remarkBreaks from 'remark-breaks'
import remarkGfm from 'remark-gfm'

type MessageMarkdownProps = {
  content: string
}

export function MessageMarkdown({ content }: MessageMarkdownProps) {
  return (
    <div className="mt-2 text-[15px] leading-7 text-[var(--rc-text-primary)]">
      <ReactMarkdown
        remarkPlugins={[remarkGfm, remarkBreaks]}
        components={{
          p: ({ children }) => <p className="my-2 whitespace-pre-wrap">{children}</p>,
          ul: ({ children }) => <ul className="my-2 list-disc space-y-1 pl-6">{children}</ul>,
          ol: ({ children }) => <ol className="my-2 list-decimal space-y-1 pl-6">{children}</ol>,
          li: ({ children }) => <li>{children}</li>,
          h1: ({ children }) => <h1 className="mt-5 mb-2 text-xl font-semibold">{children}</h1>,
          h2: ({ children }) => <h2 className="mt-4 mb-2 text-lg font-semibold">{children}</h2>,
          h3: ({ children }) => <h3 className="mt-3 mb-1 text-base font-semibold">{children}</h3>,
          a: ({ href, children }) => (
            <a href={href} target="_blank" rel="noreferrer" className="text-[var(--rc-accent)] underline underline-offset-2 hover:text-[var(--rc-accent-hover)]">
              {children}
            </a>
          ),
          code: ({ className, children }) => {
            const isBlock = className?.includes('language-')
            if (isBlock) {
              return (
                <code className="block overflow-x-auto rounded-[var(--rc-radius-lg)] bg-[var(--rc-bg-main)] p-4 font-mono text-xs text-[var(--rc-text-primary)]">
                  {children}
                </code>
              )
            }
            return <code className="rounded-md bg-[var(--rc-bg-card)] px-1.5 py-0.5 font-mono text-[12px] text-[var(--rc-secondary)]">{children}</code>
          },
          pre: ({ children }) => <pre className="my-2 overflow-x-auto">{children}</pre>,
          blockquote: ({ children }) => (
            <blockquote className="my-2 border-l-4 border-[var(--rc-accent)] bg-[var(--rc-accent-muted)] py-1 pl-3 text-[var(--rc-text-secondary)]">
              {children}
            </blockquote>
          ),
          table: ({ children }) => (
            <div className="my-2 overflow-x-auto">
              <table className="w-full border-collapse text-left text-xs">{children}</table>
            </div>
          ),
          th: ({ children }) => <th className="border border-[var(--rc-border)] bg-[var(--rc-bg-card)] px-2 py-1.5">{children}</th>,
          td: ({ children }) => <td className="border border-[var(--rc-border)] px-2 py-1.5">{children}</td>,
        }}
      >
        {content}
      </ReactMarkdown>
    </div>
  )
}
