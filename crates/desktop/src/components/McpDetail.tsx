// One MCP server, edited in place. The settings dialog is the only layer, so
// nothing here opens a dialog of its own.
//
// The draft is local: nothing is stored until Save, and the stored list is the
// panel's business. Which fields exist follows the transport — a server that is
// started as a process has a command and arguments, one reached over HTTP has a
// URL — and switching between them keeps only the name, since nothing else
// means the same thing on both.
import { useState } from 'react';
import { Trash2 } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Field, TextInput } from '@/components/ui/field';
import { t } from '@/lib/i18n';
import {
  emptyServer,
  normalize,
  refusalText,
  validateServer,
  type McpEntry,
  type McpRefusal,
  type McpServerSpec,
  type McpTransport,
} from '@/lib/mcpServers';
import { cn } from '@/lib/utils';

const TRANSPORTS: {
  transport: McpTransport;
  label: Parameters<typeof t>[0];
}[] = [
  { transport: 'stdio', label: 'mcp.transport.stdio' },
  { transport: 'http', label: 'mcp.transport.http' },
];

/** One name/value row: an environment variable, or an HTTP header. */
function EntryRows({
  entries,
  onChange,
  namePlaceholder,
  valuePlaceholder,
  addLabel,
  removeLabel,
  emptyLabel,
}: {
  entries: McpEntry[];
  onChange: (entries: McpEntry[]) => void;
  namePlaceholder: string;
  valuePlaceholder: string;
  addLabel: Parameters<typeof t>[0];
  removeLabel: Parameters<typeof t>[0];
  emptyLabel: Parameters<typeof t>[0];
}) {
  return (
    <div className="flex flex-col gap-1.5">
      {entries.length === 0 ? (
        <span className="text-xs text-muted-foreground">{t(emptyLabel)}</span>
      ) : (
        entries.map((entry, index) => (
          <div key={index} className="flex items-center gap-1.5">
            <TextInput
              value={entry.name}
              placeholder={namePlaceholder}
              onChange={(name) =>
                onChange(
                  entries.map((current, i) =>
                    i === index ? { ...current, name } : current,
                  ),
                )
              }
            />
            <TextInput
              value={entry.value}
              placeholder={valuePlaceholder}
              onChange={(value) =>
                onChange(
                  entries.map((current, i) =>
                    i === index ? { ...current, value } : current,
                  ),
                )
              }
            />
            <Button
              variant="ghost"
              size="icon"
              title={t(removeLabel)}
              aria-label={t(removeLabel)}
              onClick={() => onChange(entries.filter((_, i) => i !== index))}
            >
              <Trash2 className="h-3.5 w-3.5 text-destructive" />
            </Button>
          </div>
        ))
      )}
      <Button
        variant="outline"
        className="shrink-0 self-start"
        onClick={() => onChange([...entries, { name: '', value: '' }])}
      >
        {t(addLabel)}
      </Button>
    </div>
  );
}

/** The starting draft for a server, or for an empty one when given none. */
export function draftOf(server: McpServerSpec | null): McpServerSpec {
  return server ?? emptyServer('stdio');
}

interface McpDetailProps {
  /** The server being changed, or `null` to add one. */
  server: McpServerSpec | null;
  /** The names the other servers already carry. */
  taken: string[];
  /** The last change the agent refused, shown beside the fields that caused it. */
  refusal: McpRefusal | null;
  /** A change is on its way to the agent; the fields hold still until it lands. */
  busy?: boolean;
  onSave: (server: McpServerSpec) => void;
  /** Giving up on an add. An edit resets its own fields instead. */
  onCancel: () => void;
  /**
   * The heading above the form, `null` when the caller shows one of its own —
   * the pane that has tabs names the server above them.
   */
  title?: string | null;
}

export function McpDetail({
  server,
  taken,
  refusal,
  busy = false,
  onSave,
  onCancel,
  title,
}: McpDetailProps) {
  const [draft, setDraft] = useState(() => draftOf(server));
  const editing = server !== null;

  const clean = normalize(draft);
  const problem = validateServer(clean, taken);
  // A form nobody has filled in yet is not told what is missing; it is merely
  // not saveable. Everything else — a name that cannot be used, a duplicate, an
  // unnamed row — is said while it is still being typed.
  const incomplete =
    clean.name === '' ||
    (clean.transport === 'http' ? clean.url === '' : clean.command === '');

  function setTransport(transport: McpTransport) {
    if (transport === draft.transport) return;
    setDraft({ ...emptyServer(transport), name: draft.name });
  }

  const heading =
    title === undefined
      ? editing
        ? t('mcp.form.edit', { name: clean.name })
        : t('mcp.form.add')
      : title;

  return (
    <div className="flex min-h-0 flex-1 flex-col gap-3">
      {heading !== null && (
        <span className="shrink-0 text-sm font-semibold">{heading}</span>
      )}

      {refusal && (
        <p className="m-0 shrink-0 rounded-md bg-destructive/10 px-3 py-2 text-xs text-destructive">
          {refusalText(refusal)}
        </p>
      )}

      <div className="flex min-h-0 flex-1 flex-col gap-3 overflow-y-auto">
        <Field label={t('mcp.field.transport')} as="div">
          <div className="flex gap-2">
            {TRANSPORTS.map(({ transport, label }) => (
              <button
                key={transport}
                type="button"
                aria-pressed={draft.transport === transport}
                onClick={() => setTransport(transport)}
                className={cn(
                  'flex-1 cursor-pointer rounded-lg border px-2 py-2 text-[13px] transition-colors',
                  draft.transport === transport
                    ? 'border-accent bg-item-hover text-accent'
                    : 'border-border text-muted-foreground hover:bg-item-hover hover:text-foreground',
                )}
              >
                {t(label)}
              </button>
            ))}
          </div>
        </Field>

        <Field
          label={t('mcp.field.name')}
          hint={
            <Hint
              text={t(editing ? 'mcp.field.name.fixed' : 'mcp.field.name.hint')}
            />
          }
        >
          <TextInput
            value={draft.name}
            placeholder="my-server"
            // The name is part of every tool name this server contributes, so
            // an edit keeps it: renaming would leave the tools a session was
            // already told about pointing at nothing.
            disabled={editing}
            onChange={(name) => setDraft((current) => ({ ...current, name }))}
          />
        </Field>

        {draft.transport === 'http' ? (
          <>
            <Field label={t('mcp.field.url')}>
              <TextInput
                value={draft.url}
                placeholder="https://example.com/mcp"
                onChange={(url) => setDraft((current) => ({ ...current, url }))}
              />
            </Field>
            <Field label={t('mcp.field.headers')} as="div">
              <EntryRows
                entries={draft.headers}
                namePlaceholder="Authorization"
                valuePlaceholder="Bearer …"
                addLabel="mcp.field.headers.add"
                removeLabel="mcp.field.headers.remove"
                emptyLabel="mcp.field.empty"
                onChange={(headers) =>
                  setDraft((current) =>
                    current.transport === 'http'
                      ? { ...current, headers }
                      : current,
                  )
                }
              />
            </Field>
          </>
        ) : (
          <>
            <Field
              label={t('mcp.field.command')}
              hint={<Hint text={t('mcp.field.command.hint')} />}
            >
              <TextInput
                value={draft.command}
                placeholder="my-mcp-server"
                onChange={(command) =>
                  setDraft((current) => ({ ...current, command }))
                }
              />
            </Field>

            <Field label={t('mcp.field.args')} as="div">
              <div className="flex flex-col gap-1.5">
                {draft.args.length === 0 ? (
                  <span className="text-xs text-muted-foreground">
                    {t('mcp.field.empty')}
                  </span>
                ) : (
                  draft.args.map((arg, index) => (
                    <div key={index} className="flex items-center gap-1.5">
                      <TextInput
                        value={arg}
                        placeholder="--flag"
                        onChange={(value) =>
                          setDraft((current) =>
                            current.transport === 'stdio'
                              ? {
                                  ...current,
                                  args: current.args.map((current, i) =>
                                    i === index ? value : current,
                                  ),
                                }
                              : current,
                          )
                        }
                      />
                      <Button
                        variant="ghost"
                        size="icon"
                        title={t('mcp.field.args.remove')}
                        aria-label={t('mcp.field.args.remove')}
                        onClick={() =>
                          setDraft((current) =>
                            current.transport === 'stdio'
                              ? {
                                  ...current,
                                  args: current.args.filter(
                                    (_, i) => i !== index,
                                  ),
                                }
                              : current,
                          )
                        }
                      >
                        <Trash2 className="h-3.5 w-3.5 text-destructive" />
                      </Button>
                    </div>
                  ))
                )}
                <Button
                  variant="outline"
                  className="shrink-0 self-start"
                  onClick={() =>
                    setDraft((current) =>
                      current.transport === 'stdio'
                        ? { ...current, args: [...current.args, ''] }
                        : current,
                    )
                  }
                >
                  {t('mcp.field.args.add')}
                </Button>
              </div>
            </Field>

            <Field label={t('mcp.field.env')} as="div">
              <EntryRows
                entries={draft.env}
                namePlaceholder="NAME"
                valuePlaceholder="value"
                addLabel="mcp.field.env.add"
                removeLabel="mcp.field.env.remove"
                emptyLabel="mcp.field.empty"
                onChange={(env) =>
                  setDraft((current) =>
                    current.transport === 'stdio'
                      ? { ...current, env }
                      : current,
                  )
                }
              />
            </Field>
          </>
        )}

        {problem !== null && !incomplete && (
          <p className="m-0 shrink-0 rounded-md bg-destructive/10 px-3 py-2 text-xs text-destructive">
            {t(problem.key, problem.params)}
          </p>
        )}
      </div>

      <div className="flex shrink-0 justify-end gap-2">
        <Button
          variant="ghost"
          onClick={() => (editing ? setDraft(draftOf(server)) : onCancel())}
        >
          {t('common.cancel')}
        </Button>
        <Button
          disabled={problem !== null || busy}
          onClick={() => {
            if (problem === null) onSave(clean);
          }}
        >
          {t('mcp.form.save')}
        </Button>
      </div>
    </div>
  );
}

function Hint({ text }: { text: string }) {
  return <span className="text-[11px] text-muted-foreground">{text}</span>;
}
