import reportFixture from '../../../../contracts/v3/examples/commands/alerts.list.result.json';
import type { AlertReport, AlertSettings, ErrorCode } from '../contracts.gen';

const BYTES = /^(0|[1-9][0-9]{0,19})$/;

/**
 * Alerts on the mock transport: the contract's invented report (one low-space alert, one unobserved
 * volume, one partial-coverage root). `alerts.configure` stores and echoes the thresholds only; the mock
 * does not re-evaluate, because nothing here is measured.
 */
export function createAlertMock(now: () => Date, fail: (code: ErrorCode, message: string) => never) {
  let settings = structuredClone(reportFixture.settings) as AlertSettings;
  const report = (): AlertReport => {
    const base = structuredClone(reportFixture) as unknown as AlertReport;
    return {
      ...base, evaluated_at: now().toISOString(), settings: structuredClone(settings),
      notifications_note: settings.notifications_enabled
        ? 'Notifications are enabled, but no operating-system notification channel is wired yet; alerts show in the app only.'
        : base.notifications_note,
    };
  };
  const int = (v: unknown, min: number, max: number) => (Number.isInteger(v) && (v as number) >= min && (v as number) <= max ? (v as number) : fail('invalid_request', `expected an integer in ${min}..${max}`));
  const bytes = (v: unknown) => (typeof v === 'string' && BYTES.test(v) ? v : fail('invalid_request', 'byte quantities are decimal strings'));
  return {
    'alerts.list': (p: Record<string, unknown>): AlertReport => {
      if (Object.keys(p).length) fail('invalid_request', 'alerts.list takes no fields');
      return report();
    },
    'alerts.configure': (p: Record<string, unknown>): AlertReport => {
      const keys = ['notifications_enabled', 'low_space_free_percent', 'low_space_free_bytes', 'growth_window_days', 'growth_bytes'];
      if (Object.keys(p).length !== keys.length || keys.some((k) => !(k in p))) fail('invalid_request', 'every alert threshold is required');
      if (typeof p.notifications_enabled !== 'boolean') fail('invalid_request', 'notifications_enabled must be a boolean');
      settings = {
        notifications_enabled: p.notifications_enabled, low_space_free_percent: int(p.low_space_free_percent, 1, 50),
        low_space_free_bytes: bytes(p.low_space_free_bytes), growth_window_days: int(p.growth_window_days, 1, 90), growth_bytes: bytes(p.growth_bytes),
      };
      return report();
    },
  };
}
