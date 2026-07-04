import '@testing-library/jest-dom/vitest';
import { cleanup } from '@testing-library/react';
import { afterEach } from 'vitest';

// vitest runs with globals disabled, so Testing Library's auto-cleanup is not
// registered — unmount rendered trees after each test to avoid DOM accumulation.
afterEach(() => {
  cleanup();
});
