// The MCP servers panel: a list on the left, the chosen server edited on the
// right.
//
// The list is the agent's: it keeps it in `mcp.json` under its configuration
// directory, a file the user may also edit by hand, so every change is answered
// with the whole list and the panel renders what that says rather than what it
// hoped it said.
import { useEffect, useState } from 'react';
import { ClipboardPaste, Plus, Trash2 } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { McpDetail } from '@/components/McpDetail';
import { McpImport } from '@/components/McpImport';
import { McpServerDetail } from '@/components/McpServerDetail';
import {
  deleteMcpServer,
  importMcpServers,
  listMcpServers,
  saveMcpServer,
} from '@/lib/acp';
import { t } from '@/lib/i18n';
import {
  refusalOf,
  refusalText,
  type McpRefusal,
  type McpServerSpec,
} from '@/lib/mcpServers';
import { cn } from '@/lib/utils';

// What a row says beneath the name: how the server is reached, in the words a
// reader of the file would use.
function summaryOf(server: McpServerSpec): string {
  return server.transport === 'http'
    ? server.url
    : [server.command, ...server.args].join(' ');
}

function namesOf(servers: readonly McpServerSpec[]): string[] {
  return servers.map((server) => server.name);
}

// What an import did, in one line: a paste that replays a configuration someone
// already had both adds and replaces, and the two are different things to know.
function noticeOf(added: string[], replaced: string[]): string | null {
  const parts = [
    added.length > 0
      ? t('mcp.notice.added', { names: added.join(', ') })
      : null,
    replaced.length > 0
      ? t('mcp.notice.replaced', { names: replaced.join(', ') })
      : null,
  ].filter((part): part is string => part !== null);
  return parts.length === 0 ? null : parts.join(' ');
}

export function McpPanel() {
  // `null` until the listing comes back, so the panel can tell "nothing is
  // configured" apart from "not asked yet".
  const [servers, setServers] = useState<McpServerSpec[] | null>(null);
  // The last refusal, kept rather than rethrown: the pane is where it belongs,
  // and a failure to list is not a reason to take the dialog down.
  const [refusal, setRefusal] = useState<McpRefusal | null>(null);
  const [busy, setBusy] = useState(false);
  // Which server the detail pane shows, by position, and whether it is showing
  // one that is not in the list yet.
  const [selected, setSelected] = useState<number | null>(null);
  const [adding, setAdding] = useState(false);
  const [pasting, setPasting] = useState(false);
  // What the last import did, kept until the next change: a paste can add and
  // replace at once, and both are worth saying out loud.
  const [notice, setNotice] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    void listMcpServers()
      .then((next) => {
        if (!cancelled) setServers(next);
      })
      .catch((err: unknown) => {
        if (cancelled) return;
        setRefusal(refusalOf(err));
        setServers([]);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  async function save(server: McpServerSpec) {
    setBusy(true);
    try {
      const result = await saveMcpServer(server);
      setServers(result.servers);
      setRefusal(null);
      setNotice(null);
      setAdding(false);
      setSelected(
        result.servers.findIndex((entry) => entry.name === server.name),
      );
    } catch (err) {
      setRefusal(refusalOf(err));
    } finally {
      setBusy(false);
    }
  }

  async function importAll(incoming: McpServerSpec[]) {
    setBusy(true);
    try {
      const result = await importMcpServers(incoming);
      setServers(result.servers);
      setRefusal(null);
      setPasting(false);
      setSelected(null);
      setNotice(noticeOf(result.added, result.replaced));
    } catch (err) {
      setRefusal(refusalOf(err));
    } finally {
      setBusy(false);
    }
  }

  async function removeAt(index: number) {
    const name = servers?.[index]?.name;
    if (name === undefined) return;
    setBusy(true);
    try {
      const result = await deleteMcpServer(name);
      setServers(result.servers);
      setRefusal(null);
      setNotice(null);
      setSelected(null);
    } catch (err) {
      setRefusal(refusalOf(err));
    } finally {
      setBusy(false);
    }
  }

  const editing =
    !adding && servers !== null && selected !== null
      ? servers[selected]
      : undefined;
  // The name the form is editing is not a duplicate of itself.
  const taken = (servers ?? [])
    .filter((_, index) => adding || index !== selected)
    .map((server) => server.name);

  return (
    <div className="flex min-h-0 flex-1 flex-col gap-3">
      <span className="shrink-0 text-xs text-muted-foreground">
        {t('mcp.hint')}
      </span>

      <div className="flex min-h-0 flex-1 gap-4">
        <div className="flex w-[220px] shrink-0 flex-col gap-1.5">
          {/* Adding a server and adding a whole configuration are the same
              kind of act, so they share a row: the one server at a time keeps
              the width, the paste takes an icon beside it. */}
          <div className="flex shrink-0 items-center gap-1.5">
            <Button
              variant={adding ? 'default' : 'outline'}
              className="min-w-0 flex-1"
              aria-pressed={adding}
              onClick={() => {
                setAdding(true);
                setPasting(false);
                setNotice(null);
              }}
            >
              <Plus className="h-3.5 w-3.5" />
              {t('mcp.add')}
            </Button>
            <Button
              variant={pasting ? 'default' : 'outline'}
              size="icon"
              className="h-9 w-9 rounded-md"
              title={t('mcp.paste.open')}
              aria-label={t('mcp.paste.open')}
              aria-pressed={pasting}
              onClick={() => {
                setPasting((open) => !open);
                setAdding(false);
                setSelected(null);
                setRefusal(null);
                setNotice(null);
              }}
            >
              <ClipboardPaste className="h-3.5 w-3.5" />
            </Button>
          </div>

          {notice && (
            <p className="m-0 rounded-md bg-item-hover px-3 py-2 text-xs text-muted-foreground">
              {notice}
            </p>
          )}

          {/* The pane owns the message whenever it is open, so the same refusal
              is not printed twice. This is only for the states with no pane to
              show it — a listing that failed, say. */}
          {refusal && !adding && !pasting && !editing && (
            <p className="m-0 rounded-md bg-destructive/10 px-3 py-2 text-xs text-destructive">
              {refusalText(refusal)}
            </p>
          )}

          {servers === null ? (
            <span className="text-xs text-muted-foreground">
              {t('mcp.loading')}
            </span>
          ) : servers.length === 0 ? (
            <span className="text-xs text-muted-foreground">
              {t('mcp.empty')}
            </span>
          ) : (
            <ul className="m-0 flex min-h-0 list-none flex-col gap-1 overflow-y-auto p-0">
              {servers.map((server, index) => (
                <li
                  key={index}
                  className={cn(
                    'flex items-center gap-1 rounded-lg border border-border pr-1',
                    !adding &&
                      !pasting &&
                      selected === index &&
                      'border-accent bg-item-hover',
                  )}
                >
                  <button
                    type="button"
                    aria-current={!adding && !pasting && selected === index}
                    onClick={() => {
                      setAdding(false);
                      setPasting(false);
                      setSelected(index);
                      setRefusal(null);
                      setNotice(null);
                    }}
                    className="flex min-w-0 flex-1 cursor-pointer flex-col items-start gap-0.5 rounded-lg px-2.5 py-2 text-left outline-none transition-colors hover:bg-item-hover"
                  >
                    <span className="w-full truncate text-[13px]">
                      {server.name}
                    </span>
                    <span className="w-full truncate font-mono text-[11px] text-muted-foreground">
                      {summaryOf(server)}
                    </span>
                  </button>
                  <Button
                    variant="ghost"
                    size="icon"
                    title={t('mcp.delete')}
                    aria-label={t('mcp.delete')}
                    disabled={busy}
                    onClick={() => void removeAt(index)}
                  >
                    <Trash2 className="h-3.5 w-3.5 text-destructive" />
                  </Button>
                </li>
              ))}
            </ul>
          )}
        </div>

        <div className="flex min-w-0 flex-1 border-l border-border pl-4">
          {pasting ? (
            <McpImport
              existing={namesOf(servers ?? [])}
              refusal={refusal}
              busy={busy}
              onImport={(incoming) => void importAll(incoming)}
              onCancel={() => {
                setPasting(false);
                setRefusal(null);
              }}
            />
          ) : adding ? (
            <McpDetail
              key="new"
              server={null}
              taken={taken}
              refusal={refusal}
              busy={busy}
              onSave={(server) => void save(server)}
              onCancel={() => {
                setAdding(false);
                setRefusal(null);
              }}
            />
          ) : editing ? (
            <McpServerDetail
              // Keyed by position so a switch starts from the stored server
              // rather than from the tab of the one before it.
              key={selected ?? 'none'}
              server={editing}
              taken={taken}
              refusal={refusal}
              busy={busy}
              onSave={(server) => void save(server)}
            />
          ) : (
            <span className="text-xs text-muted-foreground">
              {servers === null ? t('mcp.loading') : t('mcp.select')}
            </span>
          )}
        </div>
      </div>
    </div>
  );
}
