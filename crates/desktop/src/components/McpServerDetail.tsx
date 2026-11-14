// One configured server: what it gives, and how it is set up.
//
// The tools come first because that is the question the list of servers raises —
// a name and a command say where a server is, not what it can do. The form is a
// click away, and its heading is suppressed: the name above the tabs is the
// heading.
import { useState } from 'react';
import { McpDetail } from '@/components/McpDetail';
import { McpTools } from '@/components/McpTools';
import { t, type Key } from '@/lib/i18n';
import { cn } from '@/lib/utils';
import type { McpRefusal, McpServerSpec } from '@/lib/mcpServers';

type Tab = 'tools' | 'settings';

const TABS: { id: Tab; label: Key }[] = [
  { id: 'tools', label: 'mcp.tab.tools' },
  { id: 'settings', label: 'mcp.tab.settings' },
];

interface McpServerDetailProps {
  server: McpServerSpec;
  /** The names the other servers already carry. */
  taken: string[];
  refusal: McpRefusal | null;
  busy?: boolean;
  onSave: (server: McpServerSpec) => void;
}

export function McpServerDetail({
  server,
  taken,
  refusal,
  busy = false,
  onSave,
}: McpServerDetailProps) {
  const [tab, setTab] = useState<Tab>('tools');

  return (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col gap-3">
      <div className="flex shrink-0 flex-col gap-1.5">
        <span className="truncate text-sm font-semibold">{server.name}</span>
        <div className="flex gap-4 border-b border-border">
          {TABS.map(({ id, label }) => (
            <button
              key={id}
              type="button"
              aria-pressed={tab === id}
              onClick={() => setTab(id)}
              className={cn(
                '-mb-px cursor-pointer border-b-2 pb-1.5 text-[13px] transition-colors outline-none focus-visible:ring-2 focus-visible:ring-accent/40',
                tab === id
                  ? 'border-accent text-foreground'
                  : 'border-transparent text-muted-foreground hover:text-foreground',
              )}
            >
              {t(label)}
            </button>
          ))}
        </div>
      </div>

      {tab === 'tools' ? (
        <McpTools server={server} />
      ) : (
        <McpDetail
          title={null}
          server={server}
          taken={taken}
          refusal={refusal}
          busy={busy}
          onSave={onSave}
          onCancel={() => {}}
        />
      )}
    </div>
  );
}
