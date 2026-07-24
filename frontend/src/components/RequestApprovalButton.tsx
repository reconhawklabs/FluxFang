// "Request approval": ask the Standalone to enrol this sensor, right now.
//
// The forwarder already retries on its own, but on a jittered ~30s schedule
// with no outward sign. After approving on the Standalone there is an
// unexplained pause before the sensor notices, which reads as a failure and
// sends people hunting for bugs that aren't there. This turns the round trip
// into something the operator triggers deliberately, so they know when to
// look and what the answer was.
//
// Shared by the Sensor dashboard and Settings so the two cannot drift.
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { api } from '../api/client';
import type { ApprovalRequestResult } from '../api/client';
import { queryKeys } from '../api/queryKeys';

export interface RequestApprovalButtonProps {
  /** Extra classes for the button, so each host page can size it to fit. */
  className?: string;
}

const TONE: Record<ApprovalRequestResult['status'], string> = {
  approved: 'text-emerald-400',
  pending: 'text-amber-400',
  not_configured: 'text-red-400',
};

export default function RequestApprovalButton({ className = '' }: RequestApprovalButtonProps) {
  const queryClient = useQueryClient();
  const mutation = useMutation({
    mutationFn: () => api.requestApproval(),
    // The attempt updates the node's forwarding health, so pull the status
    // tile back in step rather than leaving it showing the pre-click state.
    onSettled: () => void queryClient.invalidateQueries({ queryKey: queryKeys.sensorStatus }),
  });

  const result = mutation.data;

  return (
    <div className="space-y-2">
      <button
        type="button"
        data-testid="request-approval"
        disabled={mutation.isPending}
        onClick={() => mutation.mutate()}
        className={`rounded bg-amber-500 px-3 py-1.5 text-sm font-semibold text-slate-950 transition hover:bg-amber-400 disabled:cursor-not-allowed disabled:opacity-50 ${className}`}
      >
        {mutation.isPending ? 'Requesting…' : 'Request approval'}
      </button>

      {mutation.isError && (
        <p role="alert" className="text-xs text-red-400">
          Could not reach this node's own API to send the request.
        </p>
      )}

      {result && (
        <div data-testid="request-approval-result" className="space-y-1">
          <p className={`text-xs ${TONE[result.status]}`}>{result.detail}</p>
          {/* Shown only while waiting on the operator: this is the value they
              have to compare against the Standalone's approval dialog, and
              hunting for it elsewhere is the fiddliest part of enrolling. */}
          {result.status === 'pending' && result.fingerprint && (
            <p className="font-mono text-xs text-slate-400">
              {result.sensor_id} fingerprint {result.fingerprint}
            </p>
          )}
        </div>
      )}
    </div>
  );
}
