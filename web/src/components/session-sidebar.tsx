import React, { useEffect, useRef, useState } from 'react'
import { Badge, Button, Flex, ScrollArea, Separator, Text } from '@radix-ui/themes'
import type { SessionItem } from '../types'

type SessionSidebarProps = {
  appearance: 'dark' | 'light'
  onToggleAppearance: () => void
  sessionItems: SessionItem[]
  selectedSessionKey: string
  onSessionSelect: (key: string) => void
  onRefreshSession: (key: string) => void
  onResetSession: (key: string) => void
  onDeleteSession: (key: string) => void
  onOpenConfig: () => Promise<void>
  onOpenUsage: () => Promise<void>
  onNewSession: () => void
}

export function SessionSidebar({
  appearance,
  onToggleAppearance,
  sessionItems,
  selectedSessionKey,
  onSessionSelect,
  onRefreshSession,
  onResetSession,
  onDeleteSession,
  onOpenConfig,
  onOpenUsage,
  onNewSession,
}: SessionSidebarProps) {
  const isDark = appearance === 'dark'
  const [menu, setMenu] = useState<{ x: number; y: number; key: string } | null>(null)
  const sessionMenuRef = useRef<HTMLDivElement | null>(null)

  useEffect(() => {
    const onPointerDown = (event: PointerEvent) => {
      const target = event.target as Node | null
      if (!target) return
      if (sessionMenuRef.current?.contains(target)) return
      setMenu(null)
    }

    const closeOnScroll = () => {
      setMenu(null)
    }

    window.addEventListener('pointerdown', onPointerDown)
    window.addEventListener('scroll', closeOnScroll, true)
    return () => {
      window.removeEventListener('pointerdown', onPointerDown)
      window.removeEventListener('scroll', closeOnScroll, true)
    }
  }, [])

  return (
    <aside className="flex h-full min-h-0 flex-col border-r border-[var(--rc-border-subtle)] bg-[var(--rc-bg-sidebar)] p-5">
      <Flex justify="between" align="center" className="mb-5">
        <div className="flex items-center gap-2.5">
          <img
            src="/logo.png"
            alt="RayClaw"
            className="h-7 w-7 rounded-lg border border-[var(--rc-border-subtle)] object-cover"
            loading="eager"
            decoding="async"
          />
          <Text size="5" weight="bold">
            RayClaw
          </Text>
        </div>
        <button
          type="button"
          onClick={(e) => {
            e.stopPropagation()
            onToggleAppearance()
          }}
          aria-label={isDark ? 'Switch to light mode' : 'Switch to dark mode'}
          className="inline-flex h-8 w-8 items-center justify-center rounded-lg border border-[var(--rc-border)] bg-[var(--rc-bg-card)] text-[var(--rc-text-secondary)] transition-colors hover:bg-[var(--rc-bg-elevated)]"
        >
          <span className="text-sm">{isDark ? '☀' : '☾'}</span>
        </button>
      </Flex>

      <button
        type="button"
        onClick={onNewSession}
        className="mb-4 inline-flex h-9 w-full items-center justify-center rounded-lg border border-transparent text-[15px] font-medium transition hover:brightness-110 active:brightness-95"
        style={{ backgroundColor: 'var(--rc-accent)', color: isDark ? '#0d0f1a' : '#ffffff' }}
      >
        New Session
      </button>

      <Separator size="4" className="mb-4" />

      <Flex justify="between" align="center" className="mb-2">
        <Text size="2" weight="medium" color="gray">
          Sessions
        </Text>
        <Badge variant="surface">{sessionItems.length}</Badge>
      </Flex>

      <div className="min-h-0 flex-1 rounded-[var(--rc-radius-lg)] border border-[var(--rc-border-subtle)] bg-[var(--rc-bg-card)] p-2">
        <ScrollArea type="auto" style={{ height: '100%' }}>
          <div className="mb-2">
            <Text size="1" color="gray">
              Chats
            </Text>
          </div>
          <div className="flex flex-col gap-1.5 pr-1">
            {sessionItems.map((item) => (
              <button
                key={item.session_key}
                type="button"
                onClick={() => onSessionSelect(item.session_key)}
                onContextMenu={(e) => {
                  e.preventDefault()
                  setMenu({ x: e.clientX, y: e.clientY, key: item.session_key })
                }}
                className={
                  selectedSessionKey === item.session_key
                    ? 'flex w-full flex-col items-start rounded-lg border border-[var(--rc-accent)] bg-[var(--rc-bg-elevated)] px-3 py-2 text-left shadow-sm'
                    : 'flex w-full flex-col items-start rounded-lg border border-transparent px-3 py-2 text-left text-[var(--rc-text-secondary)] hover:border-[var(--rc-border)] hover:bg-[var(--rc-bg-elevated)]'
                }
              >
                <span className="max-w-[200px] truncate text-sm font-medium">{item.label}</span>
                <span className="mt-0.5 text-[11px] uppercase tracking-wide text-[var(--rc-text-tertiary)]">
                  {item.chat_type}
                </span>
              </button>
            ))}
          </div>
        </ScrollArea>
      </div>

      <div className="mt-4 border-t border-[var(--rc-border-subtle)] pt-3">
        <Button size="2" variant="soft" onClick={() => void onOpenUsage()} style={{ width: '100%' }}>
          Usage Panel
        </Button>
        <Button size="2" variant="soft" onClick={() => void onOpenConfig()} style={{ width: '100%', marginTop: '8px' }}>
          Runtime Config
        </Button>
        <div className="mt-3 flex flex-col items-center gap-1">
          <a
            href="https://rayclaw.ai"
            target="_blank"
            rel="noreferrer"
            className="text-xs text-[var(--rc-text-tertiary)] hover:text-[var(--rc-text-primary)]"
          >
            rayclaw.ai
          </a>
        </div>
      </div>

      {menu ? (
        <div
          ref={sessionMenuRef}
          className="fixed z-50 min-w-[170px] rounded-lg border border-[var(--rc-border)] bg-[var(--rc-bg-panel)] p-1.5 shadow-xl"
          style={{ left: menu.x, top: menu.y }}
          onClick={(e) => e.stopPropagation()}
        >
          <button
            type="button"
            className="flex w-full rounded-md px-3 py-2 text-left text-sm text-[var(--rc-text-primary)] hover:bg-[var(--rc-bg-elevated)]"
            onClick={() => {
              onRefreshSession(menu.key)
              setMenu(null)
            }}
          >
            Refresh
          </button>
          <button
            type="button"
            className="mt-1 flex w-full rounded-md px-3 py-2 text-left text-sm text-amber-400 hover:bg-amber-900/15"
            onClick={() => {
              onResetSession(menu.key)
              setMenu(null)
            }}
          >
            Clear Context
          </button>
          <button
            type="button"
            className="mt-1 flex w-full rounded-md px-3 py-2 text-left text-sm text-red-400 hover:bg-red-900/15"
            onClick={() => {
              onDeleteSession(menu.key)
              setMenu(null)
            }}
          >
            Delete
          </button>
        </div>
      ) : null}
    </aside>
  )
}
