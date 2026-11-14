// What one server declares, asked of the agent when the server is looked at.
//
// The list is the answer to "what does this give me", which a configuration
// alone never answers: a command and a URL say where the server is, not what it
// can do. Asking means connecting, so the three states are all shown — a server
// that is starting, one that answered, and one that could not be reached.
import { useCallback, useEffect, useState } from 'react';
import { RotateCw } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { listMcpTools } from '@/lib/acp';
import { t } from '@/lib/i18n';
import { refusalOf, type McpServerSpec, type McpTool } from '@/lib/mcpServers';

type Listing =
  | { status: 'pending' }
  | { status: 'ready'; tools: McpTool[] }
  | { status: 'failed'; reason: string };

export function McpTools({ server }: { server: McpServerSpec }) {
  const [listing, setListing] = useState<Listing>({ status: 'pending' });
  // Bumped by the retry button; the effect below is keyed on it as well as on
  // the server, so asking again is the same code path as asking the first time.
  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    let cancelled = false;
    setListing({ status: 'pending' });
    void listMcpTools(server)
      .then((tools) => {
        if (!cancelled) setListing({ status: 'ready', tools });
      })
      .catch((err: unknown) => {
        if (!cancelled) {
          setListing({ status: 'failed', reason: refusalOf(err).detail });
        }
      });
    return () => {
      cancelled = true;
    };
  }, [server, attempt]);

  const retry = useCallback(() => setAttempt((count) => count + 1), []);

  return (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col gap-2">
      <div className="flex shrink-0 items-center justify-between gap-2">
        <span className="text-xs text-muted-foreground">
          {listing.status === 'ready'
            ? listing.tools.length === 1
              ? t('mcp.tools.one')
              : t('mcp.tools.count', { count: String(listing.tools.length) })
            : listing.status === 'pending'
              ? t('mcp.tools.loading')
              : t('mcp.tools.failed')}
        </span>
        <Button
          variant="ghost"
          size="icon"
          className="h-7 w-7 rounded-md"
          title={t('mcp.tools.refresh')}
          aria-label={t('mcp.tools.refresh')}
          onClick={retry}
        >
          <RotateCw className="h-3.5 w-3.5" />
        </Button>
      </div>

      {listing.status === 'failed' ? (
        <p className="m-0 rounded-md bg-destructive/10 px-3 py-2 text-xs text-destructive">
          {listing.reason}
        </p>
      ) : listing.status === 'ready' && listing.tools.length === 0 ? (
        <p className="m-0 text-xs text-muted-foreground">
          {t('mcp.tools.empty')}
        </p>
      ) : (
        <ul className="m-0 flex min-h-0 w-full min-w-0 list-none flex-col gap-1.5 overflow-y-auto p-0">
          {listing.status === 'ready' &&
            listing.tools.map((tool) => (
              // One row per tool, and the same height whatever the server wrote:
              // a description can run to a paragraph, and a card that grows with
              // it makes two servers impossible to compare. `w-full` on the two
              // lines is what gives `truncate` a width to run out of — without
              // it a flex column sizes them to the text, and the card is dragged
              // wider than the pane.
              <li
                key={tool.name}
                className="flex h-14 w-full min-w-0 shrink-0 flex-col justify-center gap-1 overflow-hidden rounded-lg border border-border px-2.5"
              >
                {/* The name the model calls. The server's own name is a part
                    of it, which is why a server cannot be renamed in place. */}
                <span
                  className="w-full min-w-0 truncate font-mono text-[12px]"
                  title={`mcp__${server.name}__${tool.name}`}
                >
                  {tool.name}
                </span>
                {tool.description !== '' && (
                  <span
                    className="w-full min-w-0 truncate text-[11px] leading-snug text-muted-foreground"
                    title={tool.description}
                  >
                    {tool.description}
                  </span>
                )}
              </li>
            ))}
        </ul>
      )}
    </div>
  );
}
