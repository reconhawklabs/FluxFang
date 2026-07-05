// Task 9.9: manage Alert Methods (the channels a fired alert rule can
// notify through — email/webhook/in_app) and list all configured Alert
// Rules read-only (rule *creation* lives on the Entities page, Task 9.6 —
// this page only lists them, per the task brief's YAGNI).
//
// Alert Methods:
// - "Add Method" opens a type-first form: pick email/webhook/in_app, then
//   fill that type's fields. Submits `POST /api/alert-methods
//   {name,type,enabled,config}` with the full plaintext config — the
//   backend encrypts it (see `fluxfang-api::alert_methods`'s module docs).
// - The list (`GET /api/alert-methods`) never carries a secret in the
//   first place (the backend's `safe_config` allowlist drops
//   password/webhook-secret before it's ever serialized), so rendering it
//   verbatim already can't leak one; this page additionally never echoes
//   back a submitted secret anywhere after a successful create (the form
//   just closes and the list re-renders from the safe `GET` projection).
// - "Send test" -> `POST /api/alert-methods/:id/test`, rendering the
//   returned `DeliveryStatus` (Delivered / Failed + reason) inline, per
//   method, without writing a notification row (the backend route's doc
//   comment confirms this is a synchronous, non-persisted check).
// - Edit is a lightweight rename/enable-toggle (`PATCH`) — this page does
//   not offer re-entering a method's full config through an edit form
//   (that would require the user to retype the secret every time, same
//   constraint the backend's `PATCH` validation implies: a resubmitted
//   `config` must satisfy the *entire* `EmailConfig`/`WebhookConfig`
//   shape); to change config, delete and re-add.
//
// Alert Rules: read-only list from `GET /api/alert-rules`, plus a delete
// button (nice-to-have per the brief) since the backend already exposes
// `DELETE /api/alert-rules/:id`.
import { useState } from 'react';
import type { FormEvent } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { ApiError } from '../api/client';
import { queryKeys } from '../api/queryKeys';
import {
  createAlertMethod,
  deleteAlertMethod,
  listAlertMethods,
  testAlertMethod,
  updateAlertMethod,
} from '../api/alertMethods';
import type {
  AlertMethod,
  AlertMethodType,
  CreateAlertMethodInput,
  DeliveryStatus,
} from '../api/alertMethods';
import { deleteAlertRule, listAlertRules } from '../api/alertRules';
import type { AlertRule } from '../api/alertRules';

const inputClassName =
  'w-full rounded border border-slate-700 bg-slate-950 px-2 py-1.5 text-sm text-slate-100 focus:border-amber-500 focus:outline-none';
const labelClassName = 'block text-xs font-medium uppercase tracking-wide text-slate-500';
const smallButtonClassName =
  'rounded border border-slate-700 px-2 py-1 text-xs text-slate-300 transition hover:border-amber-500 hover:text-amber-400 disabled:cursor-not-allowed disabled:opacity-50';
const cancelButtonClassName =
  'rounded border border-slate-700 px-3 py-1.5 text-sm text-slate-300 transition hover:border-slate-500 hover:text-slate-100';
const submitButtonClassName =
  'rounded bg-amber-500 px-3 py-1.5 text-sm font-semibold text-slate-950 transition hover:bg-amber-400 disabled:cursor-not-allowed disabled:opacity-50';

const TYPE_BADGE_CLASSES: Record<string, string> = {
  email: 'bg-sky-500/20 text-sky-300',
  webhook: 'bg-violet-500/20 text-violet-300',
  in_app: 'bg-slate-700 text-slate-300',
};

function TypeBadge({ type }: { type: string }) {
  return (
    <span
      className={`inline-block rounded px-2 py-0.5 text-xs font-medium ${TYPE_BADGE_CLASSES[type] ?? 'bg-slate-700 text-slate-300'}`}
    >
      {type}
    </span>
  );
}

function formatTimestamp(iso: string): string {
  const date = new Date(iso);
  return Number.isNaN(date.getTime()) ? iso : date.toLocaleString();
}

interface DeliveryResultProps {
  result: DeliveryStatus;
}

/** Renders a "Send test" outcome: Delivered in green, Failed + reason in
 * red — never a bare boolean, since the reason is exactly what an operator
 * needs to fix a broken method. */
function DeliveryResult({ result }: DeliveryResultProps) {
  if (result.status === 'delivered') {
    return <span className="text-xs text-green-400">Delivered ✓</span>;
  }
  return <span className="text-xs text-red-400">Failed: {result.reason}</span>;
}

interface AddMethodFormProps {
  onCancel: () => void;
  onSubmit: (input: CreateAlertMethodInput) => void;
  submitting: boolean;
  errorMessage: string | null;
}

/** The "Add Method" form: a type dropdown (email/webhook/in_app) followed
 * by that type's fields. Each type keeps its own local state rather than
 * one shared "config" blob, since the fields (and their required-ness)
 * differ enough that a shared shape would just mean casting/optional
 * fields everywhere; `handleSubmit` assembles the right `config` object
 * for whichever `type` is currently selected. */
function AddMethodForm({ onCancel, onSubmit, submitting, errorMessage }: AddMethodFormProps) {
  const [name, setName] = useState('');
  const [type, setType] = useState<AlertMethodType>('email');
  const [enabled, setEnabled] = useState(true);

  // email fields
  const [host, setHost] = useState('');
  const [port, setPort] = useState('587');
  const [username, setUsername] = useState('');
  const [password, setPassword] = useState('');
  const [from, setFrom] = useState('');
  const [to, setTo] = useState('');
  const [tls, setTls] = useState(true);

  // webhook fields
  const [url, setUrl] = useState('');
  const [method, setMethod] = useState('POST');
  const [headersText, setHeadersText] = useState('{}');
  const [secret, setSecret] = useState('');
  const [headersError, setHeadersError] = useState<string | null>(null);

  function handleSubmit(event: FormEvent<HTMLFormElement>): void {
    event.preventDefault();
    const trimmedName = name.trim();
    if (trimmedName.length === 0) return;

    if (type === 'email') {
      onSubmit({
        name: trimmedName,
        type: 'email',
        enabled,
        config: { host, port: Number(port), username, password, from, to, tls },
      });
      return;
    }

    if (type === 'webhook') {
      let headers: Record<string, string>;
      try {
        const parsed: unknown = headersText.trim().length === 0 ? {} : JSON.parse(headersText);
        if (typeof parsed !== 'object' || parsed === null || Array.isArray(parsed)) {
          throw new Error('not an object');
        }
        headers = parsed as Record<string, string>;
      } catch {
        setHeadersError('Headers must be valid JSON (e.g. {"X-Foo":"bar"}).');
        return;
      }
      setHeadersError(null);
      onSubmit({
        name: trimmedName,
        type: 'webhook',
        enabled,
        config: { url, method, headers, secret },
      });
      return;
    }

    onSubmit({ name: trimmedName, type: 'in_app', enabled, config: {} });
  }

  return (
    <div className="fixed inset-0 z-10 flex items-center justify-center bg-slate-950/70 px-4">
      <form
        onSubmit={handleSubmit}
        className="max-h-[90vh] w-full max-w-lg space-y-4 overflow-y-auto rounded-lg border border-slate-800 bg-slate-900 p-6 shadow-xl"
      >
        <h2 className="text-lg font-semibold text-slate-100">Add Alert Method</h2>

        <div className="space-y-1">
          <label htmlFor="method-name" className={labelClassName}>
            Name
          </label>
          <input
            id="method-name"
            type="text"
            required
            autoFocus
            value={name}
            onChange={(event) => setName(event.target.value)}
            className={inputClassName}
          />
        </div>

        <div className="space-y-1">
          <label htmlFor="method-type" className={labelClassName}>
            Type
          </label>
          <select
            id="method-type"
            value={type}
            onChange={(event) => setType(event.target.value as AlertMethodType)}
            className={inputClassName}
          >
            <option value="email">Email</option>
            <option value="webhook">Webhook</option>
            <option value="in_app">In-App</option>
          </select>
        </div>

        <label className="flex items-center gap-2 text-sm text-slate-300">
          <input type="checkbox" checked={enabled} onChange={(event) => setEnabled(event.target.checked)} />
          Enabled
        </label>

        {type === 'email' && (
          <div className="space-y-3 rounded border border-slate-800 p-3">
            <div className="grid grid-cols-2 gap-3">
              <div className="space-y-1">
                <label htmlFor="method-host" className={labelClassName}>
                  Host
                </label>
                <input
                  id="method-host"
                  type="text"
                  required
                  value={host}
                  onChange={(event) => setHost(event.target.value)}
                  placeholder="smtp.example.com"
                  className={`font-mono ${inputClassName}`}
                />
              </div>
              <div className="space-y-1">
                <label htmlFor="method-port" className={labelClassName}>
                  Port
                </label>
                <input
                  id="method-port"
                  type="number"
                  required
                  value={port}
                  onChange={(event) => setPort(event.target.value)}
                  className={inputClassName}
                />
              </div>
            </div>
            <div className="space-y-1">
              <label htmlFor="method-username" className={labelClassName}>
                Username
              </label>
              <input
                id="method-username"
                type="text"
                required
                value={username}
                onChange={(event) => setUsername(event.target.value)}
                className={inputClassName}
              />
            </div>
            <div className="space-y-1">
              <label htmlFor="method-password" className={labelClassName}>
                Password
              </label>
              <input
                id="method-password"
                type="password"
                required
                value={password}
                onChange={(event) => setPassword(event.target.value)}
                className={inputClassName}
              />
            </div>
            <div className="space-y-1">
              <label htmlFor="method-from" className={labelClassName}>
                From
              </label>
              <input
                id="method-from"
                type="email"
                required
                value={from}
                onChange={(event) => setFrom(event.target.value)}
                className={inputClassName}
              />
            </div>
            <div className="space-y-1">
              <label htmlFor="method-to" className={labelClassName}>
                To
              </label>
              <input
                id="method-to"
                type="email"
                required
                value={to}
                onChange={(event) => setTo(event.target.value)}
                className={inputClassName}
              />
            </div>
            <label className="flex items-center gap-2 text-sm text-slate-300">
              <input type="checkbox" checked={tls} onChange={(event) => setTls(event.target.checked)} />
              Use TLS
            </label>
          </div>
        )}

        {type === 'webhook' && (
          <div className="space-y-3 rounded border border-slate-800 p-3">
            <div className="space-y-1">
              <label htmlFor="method-url" className={labelClassName}>
                URL
              </label>
              <input
                id="method-url"
                type="text"
                required
                value={url}
                onChange={(event) => setUrl(event.target.value)}
                placeholder="https://example.com/hook"
                className={`font-mono ${inputClassName}`}
              />
            </div>
            <div className="space-y-1">
              <label htmlFor="method-method" className={labelClassName}>
                HTTP Method
              </label>
              <select
                id="method-method"
                value={method}
                onChange={(event) => setMethod(event.target.value)}
                className={inputClassName}
              >
                <option value="POST">POST</option>
                <option value="PUT">PUT</option>
                <option value="PATCH">PATCH</option>
              </select>
            </div>
            <div className="space-y-1">
              <label htmlFor="method-headers" className={labelClassName}>
                Headers (JSON)
              </label>
              <textarea
                id="method-headers"
                value={headersText}
                onChange={(event) => setHeadersText(event.target.value)}
                className={`min-h-[4rem] font-mono ${inputClassName}`}
              />
              {headersError && <p className="text-xs text-red-400">{headersError}</p>}
            </div>
            <div className="space-y-1">
              <label htmlFor="method-secret" className={labelClassName}>
                Secret (HMAC signing key)
              </label>
              <input
                id="method-secret"
                type="password"
                value={secret}
                onChange={(event) => setSecret(event.target.value)}
                className={inputClassName}
              />
            </div>
          </div>
        )}

        {type === 'in_app' && <p className="text-sm text-slate-500">No configuration needed for in-app alerts.</p>}

        {errorMessage && (
          <p role="alert" className="text-sm text-red-400">
            {errorMessage}
          </p>
        )}

        <div className="flex justify-end gap-2 pt-2">
          <button type="button" onClick={onCancel} className={cancelButtonClassName}>
            Cancel
          </button>
          <button type="submit" disabled={submitting} className={submitButtonClassName}>
            {submitting ? 'Adding…' : 'Add'}
          </button>
        </div>
      </form>
    </div>
  );
}

interface MethodRowProps {
  method: AlertMethod;
  onTest: (id: string) => void;
  testPending: boolean;
  testResult: DeliveryStatus | undefined;
  onSaveName: (id: string, name: string) => void;
  onToggleEnabled: (method: AlertMethod) => void;
  onDelete: (method: AlertMethod) => void;
  savePending: boolean;
}

function MethodRow({
  method,
  onTest,
  testPending,
  testResult,
  onSaveName,
  onToggleEnabled,
  onDelete,
  savePending,
}: MethodRowProps) {
  const [isEditing, setIsEditing] = useState(false);
  const [nameDraft, setNameDraft] = useState(method.name);

  function handleSave(event: FormEvent<HTMLFormElement>): void {
    event.preventDefault();
    const trimmed = nameDraft.trim();
    if (trimmed.length === 0) return;
    onSaveName(method.id, trimmed);
    setIsEditing(false);
  }

  return (
    <tr data-testid={`alert-method-row-${method.id}`} className="border-b border-slate-900 align-top">
      <td className="py-2 pr-4 text-slate-200">
        {isEditing ? (
          <form onSubmit={handleSave} className="flex items-center gap-2">
            <label htmlFor={`method-name-edit-${method.id}`} className="sr-only">
              Edit name for {method.name}
            </label>
            <input
              id={`method-name-edit-${method.id}`}
              type="text"
              value={nameDraft}
              onChange={(event) => setNameDraft(event.target.value)}
              className={inputClassName}
            />
            <button type="submit" className={smallButtonClassName}>
              Save
            </button>
            <button
              type="button"
              onClick={() => {
                setIsEditing(false);
                setNameDraft(method.name);
              }}
              className={smallButtonClassName}
            >
              Cancel
            </button>
          </form>
        ) : (
          method.name
        )}
      </td>
      <td className="py-2 pr-4">
        <TypeBadge type={method.type} />
      </td>
      <td className="py-2 pr-4">
        <label className="flex items-center gap-2 text-sm text-slate-300">
          <input
            type="checkbox"
            checked={method.enabled}
            disabled={savePending}
            onChange={() => onToggleEnabled(method)}
          />
          {method.enabled ? 'Enabled' : 'Disabled'}
        </label>
      </td>
      <td className="py-2 pr-4 text-slate-300">{formatTimestamp(method.created_at)}</td>
      <td className="py-2 pr-4">
        <div className="flex flex-wrap items-center gap-2">
          {!isEditing && (
            <button type="button" onClick={() => setIsEditing(true)} className={smallButtonClassName}>
              Edit
            </button>
          )}
          <button
            type="button"
            disabled={testPending}
            onClick={() => onTest(method.id)}
            className={smallButtonClassName}
          >
            {testPending ? 'Testing…' : 'Send test'}
          </button>
          <button
            type="button"
            onClick={() => onDelete(method)}
            className="rounded border border-slate-700 px-2 py-1 text-xs text-red-400 transition hover:border-red-500"
          >
            Delete
          </button>
          {testResult && <DeliveryResult result={testResult} />}
        </div>
      </td>
    </tr>
  );
}

function AlertMethodsSection() {
  const queryClient = useQueryClient();
  const [showAddForm, setShowAddForm] = useState(false);
  const [testResults, setTestResults] = useState<Record<string, DeliveryStatus>>({});

  const methodsQuery = useQuery({ queryKey: queryKeys.alertMethods, queryFn: listAlertMethods });

  function invalidate(): void {
    void queryClient.invalidateQueries({ queryKey: queryKeys.alertMethods });
  }

  const createMutation = useMutation({
    mutationFn: (input: CreateAlertMethodInput) => createAlertMethod(input),
    onSuccess: () => {
      invalidate();
      setShowAddForm(false);
    },
  });

  const saveMutation = useMutation({
    mutationFn: ({ id, name }: { id: string; name: string }) => updateAlertMethod(id, { name }),
    onSuccess: invalidate,
  });

  const toggleMutation = useMutation({
    mutationFn: ({ id, enabled }: { id: string; enabled: boolean }) => updateAlertMethod(id, { enabled }),
    onSuccess: invalidate,
  });

  const deleteMutation = useMutation({
    mutationFn: (id: string) => deleteAlertMethod(id),
    onSuccess: invalidate,
  });

  const testMutation = useMutation({
    mutationFn: (id: string) => testAlertMethod(id),
    onSuccess: (status, id) => {
      setTestResults((prev) => ({ ...prev, [id]: status }));
    },
  });

  function handleDelete(method: AlertMethod): void {
    if (!window.confirm(`Delete alert method "${method.name}"?`)) return;
    deleteMutation.mutate(method.id);
  }

  const createErrorMessage =
    createMutation.error instanceof ApiError
      ? createMutation.error.message
      : createMutation.isError
        ? 'Failed to create alert method.'
        : null;

  const methods = methodsQuery.data ?? [];

  return (
    <section className="space-y-4">
      <div className="flex items-center justify-between">
        <h2 className="text-lg font-semibold text-slate-100">Alert Methods</h2>
        <button
          type="button"
          onClick={() => setShowAddForm(true)}
          className="rounded bg-amber-500 px-3 py-1.5 text-sm font-semibold text-slate-950 transition hover:bg-amber-400"
        >
          Add Method
        </button>
      </div>

      {methodsQuery.isLoading && <p className="text-sm text-slate-500">Loading alert methods…</p>}
      {methodsQuery.isError && <p className="text-sm text-red-400">Failed to load alert methods.</p>}
      {methodsQuery.data && methods.length === 0 && (
        <p className="text-sm text-slate-500">No alert methods configured yet.</p>
      )}

      {methods.length > 0 && (
        <table className="w-full border-collapse text-left text-sm">
          <thead>
            <tr className="border-b border-slate-800 text-xs uppercase tracking-wide text-slate-500">
              <th className="py-2 pr-4 font-medium">Name</th>
              <th className="py-2 pr-4 font-medium">Type</th>
              <th className="py-2 pr-4 font-medium">Status</th>
              <th className="py-2 pr-4 font-medium">Created</th>
              <th className="py-2 pr-4 font-medium">Actions</th>
            </tr>
          </thead>
          <tbody>
            {methods.map((method) => (
              <MethodRow
                key={method.id}
                method={method}
                onTest={(id) => testMutation.mutate(id)}
                testPending={testMutation.isPending && testMutation.variables === method.id}
                testResult={testResults[method.id]}
                onSaveName={(id, name) => saveMutation.mutate({ id, name })}
                onToggleEnabled={(m) => toggleMutation.mutate({ id: m.id, enabled: !m.enabled })}
                onDelete={handleDelete}
                savePending={toggleMutation.isPending && toggleMutation.variables?.id === method.id}
              />
            ))}
          </tbody>
        </table>
      )}

      {showAddForm && (
        <AddMethodForm
          onCancel={() => {
            setShowAddForm(false);
            createMutation.reset();
          }}
          onSubmit={(input) => createMutation.mutate(input)}
          submitting={createMutation.isPending}
          errorMessage={createErrorMessage}
        />
      )}
    </section>
  );
}

function AlertRulesSection() {
  const queryClient = useQueryClient();
  const rulesQuery = useQuery({ queryKey: queryKeys.alertRules, queryFn: listAlertRules });

  const deleteMutation = useMutation({
    mutationFn: (id: string) => deleteAlertRule(id),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.alertRules });
    },
  });

  function handleDelete(rule: AlertRule): void {
    if (!window.confirm(`Delete alert rule "${rule.name}"?`)) return;
    deleteMutation.mutate(rule.id);
  }

  const rules = rulesQuery.data ?? [];

  return (
    <section className="space-y-4">
      <h2 className="text-lg font-semibold text-slate-100">Alert Rules</h2>

      {rulesQuery.isLoading && <p className="text-sm text-slate-500">Loading alert rules…</p>}
      {rulesQuery.isError && <p className="text-sm text-red-400">Failed to load alert rules.</p>}
      {rulesQuery.data && rules.length === 0 && (
        <p className="text-sm text-slate-500">
          No alert rules yet — create one from an entity or emitter's detail view.
        </p>
      )}

      {rules.length > 0 && (
        <table className="w-full border-collapse text-left text-sm">
          <thead>
            <tr className="border-b border-slate-800 text-xs uppercase tracking-wide text-slate-500">
              <th className="py-2 pr-4 font-medium">Name</th>
              <th className="py-2 pr-4 font-medium">Target</th>
              <th className="py-2 pr-4 font-medium">Trigger</th>
              <th className="py-2 pr-4 font-medium">Methods</th>
              <th className="py-2 pr-4 font-medium">Enabled</th>
              <th className="py-2 pr-4 font-medium">Actions</th>
            </tr>
          </thead>
          <tbody>
            {rules.map((rule) => (
              <tr key={rule.id} data-testid={`alert-rule-row-${rule.id}`} className="border-b border-slate-900">
                <td className="py-2 pr-4 text-slate-200">{rule.name}</td>
                <td className="py-2 pr-4 text-slate-300">{rule.target_type ?? 'host'}</td>
                <td className="py-2 pr-4 text-slate-300">{rule.trigger.on}</td>
                <td className="py-2 pr-4 text-slate-300">{rule.method_ids.length}</td>
                <td className="py-2 pr-4 text-slate-300">{rule.enabled ? 'Yes' : 'No'}</td>
                <td className="py-2 pr-4">
                  <button
                    type="button"
                    disabled={deleteMutation.isPending && deleteMutation.variables === rule.id}
                    onClick={() => handleDelete(rule)}
                    className="rounded border border-slate-700 px-2 py-1 text-xs text-red-400 transition hover:border-red-500 disabled:cursor-not-allowed disabled:opacity-50"
                  >
                    Delete
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </section>
  );
}

export default function Alerts() {
  return (
    <div className="space-y-8">
      <h1 className="text-xl font-semibold text-slate-100">Alerts</h1>
      <AlertMethodsSection />
      <AlertRulesSection />
    </div>
  );
}
