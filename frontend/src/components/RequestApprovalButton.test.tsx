// The operator-driven enrolment request.
//
// Its whole reason to exist is removing the guesswork from an invisible ~30s
// retry, so these pin that pressing it produces a definite, readable answer
// for each outcome, and that the fingerprint an operator has to match is
// shown exactly when they need it.
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { afterEach, expect, test, vi } from 'vitest';
import RequestApprovalButton from './RequestApprovalButton';

function wrapper({ children }: { children: React.ReactNode }) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  return <QueryClientProvider client={client}>{children}</QueryClientProvider>;
}

function jsonResponse(body: unknown): Response {
  return {
    ok: true,
    status: 200,
    statusText: 'OK',
    text: () => Promise.resolve(JSON.stringify(body)),
  } as Response;
}

afterEach(() => vi.unstubAllGlobals());

test('a pending result shows the fingerprint the operator has to match', async () => {
  vi.stubGlobal(
    'fetch',
    vi.fn(() =>
      Promise.resolve(
        jsonResponse({
          status: 'pending',
          detail: 'Not approved yet. Approve this sensor on the Standalone.',
          sensor_id: 'frontgate',
          fingerprint: 'A1-B2-C3-D4-E5-F6-07-18',
        }),
      ),
    ),
  );
  render(<RequestApprovalButton />, { wrapper });
  fireEvent.click(screen.getByTestId('request-approval'));

  expect(await screen.findByText(/Approve this sensor on the Standalone/)).toBeInTheDocument();
  // The fingerprint is the fiddliest part of enrolling: it must be right here
  // rather than something the operator goes hunting for.
  expect(screen.getByText(/A1-B2-C3-D4-E5-F6-07-18/)).toBeInTheDocument();
});

test('an approved result reports success and drops the fingerprint', async () => {
  vi.stubGlobal(
    'fetch',
    vi.fn(() =>
      Promise.resolve(
        jsonResponse({
          status: 'approved',
          detail: 'Approved. Forwarding will begin on the next cycle.',
          sensor_id: 'frontgate',
          fingerprint: 'A1-B2-C3-D4-E5-F6-07-18',
        }),
      ),
    ),
  );
  render(<RequestApprovalButton />, { wrapper });
  fireEvent.click(screen.getByTestId('request-approval'));

  expect(await screen.findByText(/Forwarding will begin/)).toBeInTheDocument();
  // Nothing left to match once approved, so it would just be noise.
  expect(screen.queryByText(/A1-B2-C3-D4-E5-F6-07-18/)).not.toBeInTheDocument();
});

test('an unconfigured node is told what is missing rather than just failing', async () => {
  vi.stubGlobal(
    'fetch',
    vi.fn(() =>
      Promise.resolve(
        jsonResponse({
          status: 'not_configured',
          detail: 'This node has no usable Sensor configuration yet. Set the Standalone host, port and key in Settings first.',
        }),
      ),
    ),
  );
  render(<RequestApprovalButton />, { wrapper });
  fireEvent.click(screen.getByTestId('request-approval'));

  expect(await screen.findByText(/no usable Sensor configuration/)).toBeInTheDocument();
});

test('the button reports a failure to reach this node itself', async () => {
  vi.stubGlobal('fetch', vi.fn(() => Promise.reject(new Error('network down'))));
  render(<RequestApprovalButton />, { wrapper });
  fireEvent.click(screen.getByTestId('request-approval'));

  await waitFor(() => expect(screen.getByRole('alert')).toBeInTheDocument());
});
