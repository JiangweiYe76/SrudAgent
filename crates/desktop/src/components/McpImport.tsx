// A whole configuration pasted in at once.
//
// What people paste is the block another client's documentation prints, so the
// reader here is deliberately wider than the form: a name-keyed object, an array
// of named servers, `type` tags it does not need. Nothing is sent until the text
// reads as servers, and the panel sends all of them in one call, so a paste that
// is refused leaves the configured list exactly as it was.
import { useMemo, useState } from 'react';
import { Button } from '@/components/ui/button';
import { Field, TextArea } from '@/components/ui/field';
import { t } from '@/lib/i18n';
import {
  parsePastedServers,
  refusalText,
  type McpRefusal,
  type McpServerSpec,
} from '@/lib/mcpServers';

const SAMPLE = `{
  "mcpServers": {
    "context7": {
      "command": "npx",
      "args": ["-y", "@upstash/context7-mcp"]
    }
  }
}`;

interface McpImportProps {
  /** The names already configured, so a paste that overrides one can say so. */
  existing: readonly string[];
  /** The last change the agent refused. */
  refusal: McpRefusal | null;
  /** A change is on its way to the agent; the text holds still until it lands. */
  busy?: boolean;
  onImport: (servers: McpServerSpec[]) => void;
  onCancel: () => void;
}

export function McpImport({
  existing,
  refusal,
  busy = false,
  onImport,
  onCancel,
}: McpImportProps) {
  const [text, setText] = useState('');
  const parsed = useMemo(() => parsePastedServers(text), [text]);
  const blank = text.trim() === '';
  const ready = !blank && parsed.problems.length === 0;

  const replaces = parsed.servers
    .filter((server) =>
      existing.some((name) => name.toLowerCase() === server.name.toLowerCase()),
    )
    .map((server) => server.name);

  return (
    <div className="flex min-h-0 flex-1 flex-col gap-3">
      <span className="shrink-0 text-xs text-muted-foreground">
        {t('mcp.paste.title')}
      </span>

      {refusal && (
        <p className="m-0 shrink-0 rounded-md bg-destructive/10 px-3 py-2 text-xs text-destructive">
          {refusalText(refusal)}
        </p>
      )}

      <div className="flex min-h-0 flex-1 flex-col gap-2 overflow-y-auto">
        <Field label={t('mcp.paste.field')}>
          <TextArea
            value={text}
            rows={12}
            placeholder={SAMPLE}
            onChange={setText}
          />
        </Field>

        {parsed.problems.length > 0 ? (
          <ul className="m-0 flex list-none flex-col gap-1 p-0 text-xs text-destructive">
            {parsed.problems.map((problem, index) => (
              <li key={index}>{t(problem.key, problem.params)}</li>
            ))}
          </ul>
        ) : ready ? (
          <div className="flex flex-col gap-1 text-xs text-muted-foreground">
            <span>
              {t('mcp.paste.ready', {
                names: parsed.servers.map((server) => server.name).join(', '),
              })}
            </span>
            {replaces.length > 0 && (
              <span>
                {t('mcp.paste.replaces', { names: replaces.join(', ') })}
              </span>
            )}
          </div>
        ) : null}
      </div>

      <div className="flex shrink-0 justify-end gap-2">
        <Button variant="ghost" onClick={onCancel}>
          {t('mcp.paste.cancel')}
        </Button>
        <Button
          disabled={!ready || busy}
          onClick={() => onImport(parsed.servers)}
        >
          {t('mcp.paste.import')}
        </Button>
      </div>
    </div>
  );
}
