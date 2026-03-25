import React, { useCallback, useEffect, useState } from 'react'
import { Badge, Button, Card, Flex, ScrollArea, Separator, Tabs, Text } from '@radix-ui/themes'

// --- Types ---

type TaskItem = {
  id: number
  chat_id: number
  prompt: string
  schedule_type: string
  schedule_value: string
  next_run: string
  last_run: string | null
  status: string
  created_at: string
}

type TasksSummary = {
  total: number
  active: number
  paused: number
  completed: number
  cancelled: number
  runs_24h: number
  failures_24h: number
}

type TaskLog = {
  id: number
  started_at: string
  finished_at: string
  duration_ms: number
  success: boolean
  result_summary: string | null
}

type MemoryItem = {
  id: number
  chat_id: number | null
  content: string
  category: string
  confidence: number
  source: string
  created_at: string
  updated_at: string
  last_seen_at: string
  is_archived: boolean
  archived_at: string | null
}

type DbStats = {
  chats: number
  messages: number
  memories: number
  tasks: number
  db_size_bytes: number
}

type SessionInfo = {
  session_key: string
  label: string
  chat_id: number
  chat_type: string
}

// --- Helpers ---

async function dashApi<T>(path: string): Promise<T> {
  const res = await fetch(path)
  const data = (await res.json().catch(() => ({}))) as Record<string, unknown>
  if (!res.ok) throw new Error(String(data.error || `HTTP ${res.status}`))
  return data as T
}

function relativeTime(iso: string | null | undefined): string {
  if (!iso) return '-'
  const diff = Date.now() - Date.parse(iso)
  if (!Number.isFinite(diff)) return iso
  const abs = Math.abs(diff)
  const future = diff < 0
  let text: string
  if (abs < 60_000) text = 'just now'
  else if (abs < 3_600_000) text = `${Math.floor(abs / 60_000)}m`
  else if (abs < 86_400_000) text = `${Math.floor(abs / 3_600_000)}h`
  else text = `${Math.floor(abs / 86_400_000)}d`
  if (text === 'just now') return text
  return future ? `in ${text}` : `${text} ago`
}

function truncate(s: string, n: number): string {
  return s.length > n ? s.slice(0, n) + '...' : s
}

function fmtBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`
}

function statusColor(status: string): 'green' | 'orange' | 'gray' | 'red' {
  switch (status) {
    case 'active': return 'green'
    case 'paused': return 'orange'
    case 'completed': return 'gray'
    case 'cancelled': return 'red'
    default: return 'gray'
  }
}

function categoryColor(cat: string): 'blue' | 'purple' | 'orange' | 'gray' {
  switch (cat.toUpperCase()) {
    case 'PROFILE': return 'blue'
    case 'KNOWLEDGE': return 'purple'
    case 'EVENT': return 'orange'
    default: return 'gray'
  }
}

// --- Overview Tab ---

function OverviewTab({ sessions }: { sessions: SessionInfo[] }) {
  const [summary, setSummary] = useState<TasksSummary | null>(null)
  const [dbStats, setDbStats] = useState<DbStats | null>(null)
  const [health, setHealth] = useState<Record<string, unknown> | null>(null)
  const [loading, setLoading] = useState(true)

  const load = useCallback(async () => {
    setLoading(true)
    try {
      const [s, d, h] = await Promise.all([
        dashApi<TasksSummary>('/api/dashboard/tasks/summary'),
        dashApi<DbStats>('/api/dashboard/db/stats'),
        dashApi<Record<string, unknown>>('/api/health'),
      ])
      setSummary(s)
      setDbStats(d)
      setHealth(h)
    } catch { /* ignore */ }
    setLoading(false)
  }, [])

  useEffect(() => { void load() }, [load])

  if (loading && !summary) {
    return <Text size="2" color="gray" className="p-4">Loading...</Text>
  }

  const channelCounts: Record<string, number> = {}
  for (const s of sessions) {
    const ch = s.chat_type || 'unknown'
    channelCounts[ch] = (channelCounts[ch] || 0) + 1
  }

  return (
    <div className="space-y-4 p-1">
      <Flex gap="3" wrap="wrap">
        <StatCard label="Version" value={String(health?.version || '-')} />
        <StatCard label="Sessions" value={String(sessions.length)} />
        <StatCard label="Tasks (active)" value={String(summary?.active || 0)} sub={`${summary?.total || 0} total`} />
        <StatCard label="Runs (24h)" value={String(summary?.runs_24h || 0)} sub={`${summary?.failures_24h || 0} failed`} />
      </Flex>
      <Flex gap="3" wrap="wrap">
        <StatCard label="Messages" value={String(dbStats?.messages || 0)} />
        <StatCard label="Memories" value={String(dbStats?.memories || 0)} />
        <StatCard label="DB Size" value={fmtBytes(dbStats?.db_size_bytes || 0)} />
        <StatCard label="Chats" value={String(dbStats?.chats || 0)} />
      </Flex>
      {Object.keys(channelCounts).length > 0 && (
        <Card className="p-3">
          <Text size="2" weight="medium" className="mb-2 block">Sessions by Channel</Text>
          <Flex gap="2" wrap="wrap">
            {Object.entries(channelCounts).map(([ch, n]) => (
              <Badge key={ch} variant="surface" size="2">{ch}: {n}</Badge>
            ))}
          </Flex>
        </Card>
      )}
      <Flex justify="end">
        <Button size="1" variant="soft" onClick={() => void load()}>Refresh</Button>
      </Flex>
    </div>
  )
}

function StatCard({ label, value, sub }: { label: string; value: string; sub?: string }) {
  return (
    <Card className="min-w-[140px] flex-1 p-3">
      <Text size="1" color="gray" className="block">{label}</Text>
      <Text size="5" weight="bold" className="block">{value}</Text>
      {sub && <Text size="1" color="gray">{sub}</Text>}
    </Card>
  )
}

// --- Tasks Tab ---

function TasksTab({ sessions }: { sessions: SessionInfo[] }) {
  const [tasks, setTasks] = useState<TaskItem[]>([])
  const [total, setTotal] = useState(0)
  const [offset, setOffset] = useState(0)
  const [statusFilter, setStatusFilter] = useState('')
  const [typeFilter, setTypeFilter] = useState('')
  const [expandedId, setExpandedId] = useState<number | null>(null)
  const [logs, setLogs] = useState<TaskLog[]>([])
  const [loading, setLoading] = useState(true)
  const limit = 30

  const chatLabel = useCallback((chatId: number) => {
    const s = sessions.find(s => s.chat_id === chatId)
    return s ? `${s.chat_type}/${s.label}` : `#${chatId}`
  }, [sessions])

  const load = useCallback(async () => {
    setLoading(true)
    try {
      const params = new URLSearchParams()
      if (statusFilter) params.set('status', statusFilter)
      if (typeFilter) params.set('type', typeFilter)
      params.set('limit', String(limit))
      params.set('offset', String(offset))
      const data = await dashApi<{ tasks: TaskItem[]; total: number }>(`/api/dashboard/tasks?${params}`)
      setTasks(data.tasks || [])
      setTotal(data.total || 0)
    } catch { /* ignore */ }
    setLoading(false)
  }, [statusFilter, typeFilter, offset])

  useEffect(() => { void load() }, [load])

  const toggleExpand = async (id: number) => {
    if (expandedId === id) { setExpandedId(null); return }
    setExpandedId(id)
    try {
      const data = await dashApi<{ logs: TaskLog[] }>(`/api/dashboard/tasks/${id}/logs?limit=10`)
      setLogs(data.logs || [])
    } catch { setLogs([]) }
  }

  return (
    <div className="space-y-3 p-1">
      <Flex gap="2" align="center" wrap="wrap">
        <FilterSelect label="Status" value={statusFilter} onChange={v => { setStatusFilter(v); setOffset(0) }}
          options={['', 'active', 'paused', 'completed', 'cancelled']} />
        <FilterSelect label="Type" value={typeFilter} onChange={v => { setTypeFilter(v); setOffset(0) }}
          options={['', 'cron', 'once']} />
        <Text size="1" color="gray">{total} task{total !== 1 ? 's' : ''}</Text>
        <div className="flex-1" />
        <Button size="1" variant="soft" onClick={() => void load()}>Refresh</Button>
      </Flex>

      {loading && tasks.length === 0 ? (
        <Text size="2" color="gray">Loading...</Text>
      ) : tasks.length === 0 ? (
        <Text size="2" color="gray">No tasks found.</Text>
      ) : (
        <div className="overflow-auto rounded-lg border border-[var(--rc-border-subtle)]">
          <table className="w-full text-sm">
            <thead>
              <tr className="border-b border-[var(--rc-border-subtle)] bg-[var(--rc-bg-card)]">
                <Th>Status</Th><Th>Type</Th><Th>Schedule</Th><Th>Next Run</Th><Th>Last Run</Th><Th>Chat</Th><Th>Prompt</Th>
              </tr>
            </thead>
            <tbody>
              {tasks.map(t => (
                <React.Fragment key={t.id}>
                  <tr className="border-b border-[var(--rc-border-subtle)] hover:bg-[var(--rc-bg-elevated)] cursor-pointer"
                      onClick={() => void toggleExpand(t.id)}>
                    <Td><Badge color={statusColor(t.status)} variant="soft" size="1">{t.status}</Badge></Td>
                    <Td><Badge variant="outline" size="1">{t.schedule_type}</Badge></Td>
                    <Td><code className="text-xs">{t.schedule_value}</code></Td>
                    <Td>{relativeTime(t.next_run)}</Td>
                    <Td>{relativeTime(t.last_run)}</Td>
                    <Td><Text size="1" color="gray">{chatLabel(t.chat_id)}</Text></Td>
                    <Td><Text size="1">{truncate(t.prompt, 60)}</Text></Td>
                  </tr>
                  {expandedId === t.id && (
                    <tr><td colSpan={7} className="bg-[var(--rc-bg-card)] px-4 py-3">
                      <Text size="1" weight="medium" className="mb-1 block">Prompt</Text>
                      <pre className="mb-3 whitespace-pre-wrap text-xs text-[var(--rc-text-secondary)]">{t.prompt}</pre>
                      <Text size="1" weight="medium" className="mb-1 block">Recent Runs</Text>
                      {logs.length === 0 ? <Text size="1" color="gray">No runs yet.</Text> : (
                        <div className="space-y-1">
                          {logs.map(l => (
                            <Flex key={l.id} gap="2" align="center">
                              <Badge color={l.success ? 'green' : 'red'} size="1" variant="soft">{l.success ? 'OK' : 'FAIL'}</Badge>
                              <Text size="1">{relativeTime(l.started_at)}</Text>
                              <Text size="1" color="gray">{l.duration_ms}ms</Text>
                              {l.result_summary && <Text size="1" color="gray">{truncate(l.result_summary, 80)}</Text>}
                            </Flex>
                          ))}
                        </div>
                      )}
                    </td></tr>
                  )}
                </React.Fragment>
              ))}
            </tbody>
          </table>
        </div>
      )}

      {total > limit && (
        <Flex gap="2" justify="center" align="center">
          <Button size="1" variant="soft" disabled={offset === 0} onClick={() => setOffset(Math.max(0, offset - limit))}>Prev</Button>
          <Text size="1" color="gray">{offset + 1}-{Math.min(offset + limit, total)} of {total}</Text>
          <Button size="1" variant="soft" disabled={offset + limit >= total} onClick={() => setOffset(offset + limit)}>Next</Button>
        </Flex>
      )}
    </div>
  )
}

// --- Memories Tab ---

function MemoriesTab({ sessions }: { sessions: SessionInfo[] }) {
  const [memories, setMemories] = useState<MemoryItem[]>([])
  const [total, setTotal] = useState(0)
  const [offset, setOffset] = useState(0)
  const [categoryFilter, setCategoryFilter] = useState('')
  const [scopeFilter, setScopeFilter] = useState('')
  const [includeArchived, setIncludeArchived] = useState(false)
  const [search, setSearch] = useState('')
  const [searchInput, setSearchInput] = useState('')
  const [expandedId, setExpandedId] = useState<number | null>(null)
  const [loading, setLoading] = useState(true)
  const limit = 30

  const chatLabel = useCallback((chatId: number | null) => {
    if (chatId == null) return 'Global'
    const s = sessions.find(s => s.chat_id === chatId)
    return s ? `${s.chat_type}/${s.label}` : `#${chatId}`
  }, [sessions])

  const load = useCallback(async () => {
    setLoading(true)
    try {
      const params = new URLSearchParams()
      if (categoryFilter) params.set('category', categoryFilter)
      if (scopeFilter === 'global') params.set('chat_id', '0')
      if (includeArchived) params.set('archived', 'true')
      if (search) params.set('search', search)
      params.set('limit', String(limit))
      params.set('offset', String(offset))
      const data = await dashApi<{ memories: MemoryItem[]; total: number }>(`/api/dashboard/memories?${params}`)
      setMemories(data.memories || [])
      setTotal(data.total || 0)
    } catch { /* ignore */ }
    setLoading(false)
  }, [categoryFilter, scopeFilter, includeArchived, search, offset])

  useEffect(() => { void load() }, [load])

  return (
    <div className="space-y-3 p-1">
      <Flex gap="2" align="center" wrap="wrap">
        <FilterSelect label="Category" value={categoryFilter} onChange={v => { setCategoryFilter(v); setOffset(0) }}
          options={['', 'PROFILE', 'KNOWLEDGE', 'EVENT']} />
        <FilterSelect label="Scope" value={scopeFilter} onChange={v => { setScopeFilter(v); setOffset(0) }}
          options={['', 'global']} />
        <label className="flex items-center gap-1 text-xs text-[var(--rc-text-secondary)]">
          <input type="checkbox" checked={includeArchived} onChange={e => { setIncludeArchived(e.target.checked); setOffset(0) }} />
          Archived
        </label>
        <form onSubmit={e => { e.preventDefault(); setSearch(searchInput); setOffset(0) }} className="flex items-center gap-1">
          <input
            type="text" value={searchInput} onChange={e => setSearchInput(e.target.value)}
            placeholder="Search..."
            className="h-7 rounded border border-[var(--rc-border)] bg-transparent px-2 text-xs text-[var(--rc-text-primary)] outline-none"
          />
          <Button size="1" variant="soft" type="submit">Go</Button>
        </form>
        <Text size="1" color="gray">{total} memor{total !== 1 ? 'ies' : 'y'}</Text>
        <div className="flex-1" />
        <Button size="1" variant="soft" onClick={() => void load()}>Refresh</Button>
      </Flex>

      {loading && memories.length === 0 ? (
        <Text size="2" color="gray">Loading...</Text>
      ) : memories.length === 0 ? (
        <Text size="2" color="gray">No memories found.</Text>
      ) : (
        <div className="overflow-auto rounded-lg border border-[var(--rc-border-subtle)]">
          <table className="w-full text-sm">
            <thead>
              <tr className="border-b border-[var(--rc-border-subtle)] bg-[var(--rc-bg-card)]">
                <Th>Content</Th><Th>Category</Th><Th>Scope</Th><Th>Confidence</Th><Th>Source</Th><Th>Updated</Th><Th>Status</Th>
              </tr>
            </thead>
            <tbody>
              {memories.map(m => (
                <React.Fragment key={m.id}>
                  <tr className="border-b border-[var(--rc-border-subtle)] hover:bg-[var(--rc-bg-elevated)] cursor-pointer"
                      onClick={() => setExpandedId(expandedId === m.id ? null : m.id)}>
                    <Td><Text size="1">{truncate(m.content, 60)}</Text></Td>
                    <Td><Badge color={categoryColor(m.category)} variant="soft" size="1">{m.category}</Badge></Td>
                    <Td><Text size="1" color="gray">{chatLabel(m.chat_id)}</Text></Td>
                    <Td>
                      <Flex align="center" gap="1">
                        <div className="h-1.5 w-16 overflow-hidden rounded-full bg-[var(--rc-border-subtle)]">
                          <div className="h-full rounded-full bg-[var(--rc-accent)]" style={{ width: `${Math.round(m.confidence * 100)}%` }} />
                        </div>
                        <Text size="1" color="gray">{m.confidence.toFixed(2)}</Text>
                      </Flex>
                    </Td>
                    <Td><Text size="1" color="gray">{m.source}</Text></Td>
                    <Td><Text size="1" color="gray">{relativeTime(m.updated_at)}</Text></Td>
                    <Td><Badge color={m.is_archived ? 'gray' : 'green'} variant="soft" size="1">{m.is_archived ? 'archived' : 'active'}</Badge></Td>
                  </tr>
                  {expandedId === m.id && (
                    <tr><td colSpan={7} className="bg-[var(--rc-bg-card)] px-4 py-3">
                      <pre className="whitespace-pre-wrap text-xs text-[var(--rc-text-secondary)]">{m.content}</pre>
                      <Flex gap="3" className="mt-2">
                        <Text size="1" color="gray">Created: {relativeTime(m.created_at)}</Text>
                        <Text size="1" color="gray">Last seen: {relativeTime(m.last_seen_at)}</Text>
                        {m.archived_at && <Text size="1" color="gray">Archived: {relativeTime(m.archived_at)}</Text>}
                      </Flex>
                    </td></tr>
                  )}
                </React.Fragment>
              ))}
            </tbody>
          </table>
        </div>
      )}

      {total > limit && (
        <Flex gap="2" justify="center" align="center">
          <Button size="1" variant="soft" disabled={offset === 0} onClick={() => setOffset(Math.max(0, offset - limit))}>Prev</Button>
          <Text size="1" color="gray">{offset + 1}-{Math.min(offset + limit, total)} of {total}</Text>
          <Button size="1" variant="soft" disabled={offset + limit >= total} onClick={() => setOffset(offset + limit)}>Next</Button>
        </Flex>
      )}
    </div>
  )
}

// --- Shared table elements ---

function Th({ children }: { children: React.ReactNode }) {
  return <th className="px-3 py-2 text-left text-xs font-medium text-[var(--rc-text-secondary)]">{children}</th>
}

function Td({ children }: { children: React.ReactNode }) {
  return <td className="px-3 py-2">{children}</td>
}

function FilterSelect({ label, value, onChange, options }: {
  label: string; value: string; onChange: (v: string) => void; options: string[]
}) {
  return (
    <label className="flex items-center gap-1 text-xs text-[var(--rc-text-secondary)]">
      {label}:
      <select value={value} onChange={e => onChange(e.target.value)}
        className="h-7 rounded border border-[var(--rc-border)] bg-transparent px-1 text-xs text-[var(--rc-text-primary)] outline-none">
        {options.map(o => <option key={o} value={o}>{o || 'All'}</option>)}
      </select>
    </label>
  )
}

// --- Main Dashboard ---

type DashboardProps = {
  sessions: SessionInfo[]
  onClose: () => void
}

export function Dashboard({ sessions, onClose }: DashboardProps) {
  return (
    <div className="flex h-full min-h-0 flex-col">
      <header className="flex items-center justify-between border-b border-[var(--rc-border)] px-4 py-3">
        <Text size="5" weight="bold">Dashboard</Text>
        <Button size="1" variant="ghost" onClick={onClose}>Back to Chat</Button>
      </header>
      <div className="min-h-0 flex-1">
        <Tabs.Root defaultValue="overview" className="flex h-full min-h-0 flex-col">
          <Tabs.List className="shrink-0 px-4">
            <Tabs.Trigger value="overview">Overview</Tabs.Trigger>
            <Tabs.Trigger value="tasks">Tasks</Tabs.Trigger>
            <Tabs.Trigger value="memories">Memories</Tabs.Trigger>
          </Tabs.List>
          <ScrollArea className="min-h-0 flex-1">
            <div className="p-4">
              <Tabs.Content value="overview"><OverviewTab sessions={sessions} /></Tabs.Content>
              <Tabs.Content value="tasks"><TasksTab sessions={sessions} /></Tabs.Content>
              <Tabs.Content value="memories"><MemoriesTab sessions={sessions} /></Tabs.Content>
            </div>
          </ScrollArea>
        </Tabs.Root>
      </div>
    </div>
  )
}
