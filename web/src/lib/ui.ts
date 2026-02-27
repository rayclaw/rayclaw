import { cva } from 'class-variance-authority'

export const messageBubbleVariants = cva(
  'w-[min(88%,860px)] rounded-[10px] border px-4 py-3 shadow-[var(--rc-shadow-sm)]',
  {
  variants: {
    role: {
      bot: 'border-[var(--rc-border)] bg-[var(--rc-bg-card)]',
      user: 'border-[var(--rc-border)] bg-[var(--rc-bg-elevated)]',
    },
  },
  defaultVariants: {
    role: 'bot',
  },
  },
)
