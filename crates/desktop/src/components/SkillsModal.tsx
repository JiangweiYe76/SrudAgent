import { useEffect, useState, type ReactNode } from 'react';
import { File, Loader2, Sparkles, TriangleAlert } from 'lucide-react';
import { cn } from '@/lib/utils';
import { t } from '@/lib/i18n';
import {
  listSkills,
  readSkill,
  type SkillConflict,
  type SkillDetail,
  type SkillInfo,
} from '@/lib/skills';
import { Modal, ModalContent } from '@/components/ui/modal';
import { Markdown } from '@/components/Markdown';

interface SkillsModalProps {
  open: boolean;
  // The active session's workspace; empty asks for user-wide skills only.
  cwd: string;
  onClose: () => void;
}

/**
 * A read-only view of the skills the agent currently discovers.
 *
 * The list is loaded each time the modal opens, with any name conflicts the
 * discovery pass reported, and the selected skill is read on demand: a body and
 * its bundled files cost more than the catalog line that brought the user here.
 */
export function SkillsModal({ open, cwd, onClose }: SkillsModalProps) {
  const [skills, setSkills] = useState<SkillInfo[] | null>(null);
  const [conflicts, setConflicts] = useState<SkillConflict[]>([]);
  const [listFailed, setListFailed] = useState(false);
  const [selected, setSelected] = useState<string | null>(null);
  const [detail, setDetail] = useState<SkillDetail | null>(null);
  const [detailFailed, setDetailFailed] = useState(false);

  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    setSkills(null);
    setConflicts([]);
    setListFailed(false);
    setSelected(null);
    setDetail(null);
    setDetailFailed(false);
    void listSkills(cwd)
      .then((next) => {
        if (cancelled) return;
        setSkills(next.skills);
        setConflicts(next.conflicts);
        setSelected(next.skills[0]?.name ?? null);
      })
      .catch(() => {
        if (!cancelled) setListFailed(true);
      });
    return () => {
      cancelled = true;
    };
  }, [open, cwd]);

  useEffect(() => {
    if (!open || selected === null) return;
    let cancelled = false;
    setDetail(null);
    setDetailFailed(false);
    void readSkill(cwd, selected)
      .then((next) => {
        if (!cancelled) setDetail(next);
      })
      .catch(() => {
        if (!cancelled) setDetailFailed(true);
      });
    return () => {
      cancelled = true;
    };
  }, [open, cwd, selected]);

  const loadingList = skills === null && !listFailed;

  return (
    <Modal open={open} onOpenChange={(next) => !next && onClose()}>
      <ModalContent title={t('skills.title')} className="h-[640px] w-[960px]">
        {loadingList && (
          <CenteredStatus icon="loading" text={t('skills.loading')} />
        )}

        {listFailed && (
          <CenteredStatus icon="empty" text={t('skills.loadFailed')} />
        )}

        {skills?.length === 0 && (
          <CenteredStatus
            icon="empty"
            text={t('skills.empty')}
            hint={t('skills.emptyHint')}
          />
        )}

        {skills && skills.length > 0 && (
          <div className="flex min-h-0 flex-1 flex-col">
            {conflicts.length > 0 && <ConflictBanner conflicts={conflicts} />}
            <div className="flex min-h-0 flex-1">
              <div className="w-[300px] shrink-0 overflow-y-auto border-r border-border bg-muted/40 p-2">
                {skills.map((skill) => (
                  <button
                    key={skill.path}
                    className={cn(
                      'flex w-full cursor-pointer flex-col gap-0.5 rounded-lg px-3 py-2 text-left transition-colors',
                      selected === skill.name
                        ? 'bg-item-hover'
                        : 'hover:bg-item-hover',
                    )}
                    onClick={() => setSelected(skill.name)}
                    aria-current={selected === skill.name}
                  >
                    <span className="w-full truncate text-[13px] font-medium">
                      {skill.name}
                    </span>
                    <span className="w-full truncate text-[11px] text-muted-foreground">
                      {skill.description}
                    </span>
                  </button>
                ))}
              </div>

              <div className="flex min-w-0 flex-1 flex-col overflow-y-auto">
                {selected === null && (
                  <CenteredStatus icon="empty" text={t('skills.select')} />
                )}
                {selected !== null && detail === null && !detailFailed && (
                  <CenteredStatus
                    icon="loading"
                    text={t('skills.detailLoading')}
                  />
                )}
                {detailFailed && (
                  <CenteredStatus
                    icon="empty"
                    text={t('skills.detailFailed')}
                  />
                )}
                {detail !== null && <SkillDetailPane skill={detail} />}
              </div>
            </div>
          </div>
        )}
      </ModalContent>
    </Modal>
  );
}

function ConflictBanner({ conflicts }: { conflicts: SkillConflict[] }) {
  return (
    <div className="shrink-0 border-b border-border bg-muted px-4 py-2 text-[11px] text-muted-foreground">
      <div className="flex items-center gap-1.5">
        <TriangleAlert className="h-3.5 w-3.5 shrink-0" />
        <span className="font-medium text-foreground">
          {t('skills.conflicts', { n: conflicts.length })}
        </span>
        <span>{t('skills.conflictsHint')}</span>
      </div>
      <ul className="m-0 mt-1 flex list-none flex-col gap-0.5 p-0">
        {conflicts.map((conflict) => (
          <li
            key={`${conflict.name}:${conflict.loserPath}`}
            className="truncate font-mono"
            title={`${conflict.loserPath} → ${conflict.winnerPath}`}
          >
            {conflict.name}: {conflict.loserPath} → {conflict.winnerPath}
          </li>
        ))}
      </ul>
    </div>
  );
}

function SkillDetailPane({ skill }: { skill: SkillDetail }) {
  const visibility = [
    skill.modelInvocable ? t('skills.modelVisible') : null,
    skill.userInvocable ? t('skills.userVisible') : null,
  ]
    .filter((value): value is string => value !== null)
    .join(' · ');

  return (
    <div className="flex flex-col gap-5 p-5">
      <div className="flex flex-col gap-1.5">
        <div className="flex min-w-0 items-center gap-2">
          <h2 className="m-0 truncate text-[15px] font-semibold">
            {skill.name}
          </h2>
          {!skill.modelInvocable && <Badge>{t('skills.hidden')}</Badge>}
          {!skill.userInvocable && <Badge>{t('skills.modelOnly')}</Badge>}
        </div>
        <p className="m-0 text-[13px] text-muted-foreground">
          {skill.description}
        </p>
        <span className="break-all font-mono text-[11px] text-muted-foreground/70">
          {skill.path}
        </span>
      </div>

      <Section title={t('skills.fields')}>
        <Field label={t('skills.license')} value={skill.license} />
        <Field label={t('skills.compatibility')} value={skill.compatibility} />
        <Field
          label={t('skills.allowedTools')}
          value={skill.allowedTools.join(' · ')}
        />
        <Field label={t('skills.visibility')} value={visibility} />
      </Section>

      {Object.keys(skill.metadata).length > 0 && (
        <Section title={t('skills.metadata')}>
          {Object.entries(skill.metadata).map(([key, value]) => (
            <Field
              key={key}
              label={key}
              value={typeof value === 'string' ? value : JSON.stringify(value)}
            />
          ))}
        </Section>
      )}

      <Section title={t('skills.files')}>
        {skill.supportingFiles.length === 0 ? (
          <Muted>{t('skills.noFiles')}</Muted>
        ) : (
          <ul className="m-0 flex list-none flex-col gap-1 p-0">
            {skill.supportingFiles.map((file) => (
              <li
                key={file}
                className="flex min-w-0 items-center gap-2 font-mono text-[11px] text-muted-foreground"
              >
                <File className="h-3.5 w-3.5 shrink-0" />
                <span className="truncate">{file}</span>
              </li>
            ))}
          </ul>
        )}
      </Section>

      <Section title={t('skills.instructions')}>
        {skill.body.trim() === '' ? (
          <Muted>{t('skills.noInstructions')}</Muted>
        ) : (
          <Markdown className="text-[13px]">{skill.body}</Markdown>
        )}
      </Section>
    </div>
  );
}

function Section({ title, children }: { title: string; children: ReactNode }) {
  return (
    <div className="flex flex-col gap-2">
      <h3 className="m-0 text-[11px] font-semibold tracking-wide text-muted-foreground uppercase">
        {title}
      </h3>
      {children}
    </div>
  );
}

function Field({ label, value }: { label: string; value?: string }) {
  if (!value) return null;
  return (
    <div className="flex gap-3 text-[12px]">
      <span className="w-28 shrink-0 text-muted-foreground">{label}</span>
      <span className="min-w-0 break-words">{value}</span>
    </div>
  );
}

function Badge({ children }: { children: ReactNode }) {
  return (
    <span className="shrink-0 rounded-full border border-border px-2 py-0.5 text-[10px] text-muted-foreground">
      {children}
    </span>
  );
}

function Muted({ children }: { children: ReactNode }) {
  return <p className="m-0 text-[12px] text-muted-foreground">{children}</p>;
}

function CenteredStatus({
  icon,
  text,
  hint,
}: {
  icon: 'loading' | 'empty';
  text: string;
  hint?: string;
}) {
  return (
    <div className="flex flex-1 flex-col items-center justify-center gap-2 px-6 py-10 text-center text-muted-foreground">
      {icon === 'loading' ? (
        <Loader2 className="h-5 w-5 animate-spin" />
      ) : (
        <Sparkles className="h-5 w-5" />
      )}
      <p className="m-0 text-[13px]">{text}</p>
      {hint && <p className="m-0 text-[11px]">{hint}</p>}
    </div>
  );
}
