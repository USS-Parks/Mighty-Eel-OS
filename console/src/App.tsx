import { Navigate, Route, Routes } from 'react-router-dom';
import { useAuth } from './auth/AuthContext';
import { LoginView } from './auth/LoginView';
import { AppShell } from './components/AppShell';
import { OverviewView } from './views/OverviewView';
import { RoutingView } from './views/RoutingView';
import { AuditView } from './views/AuditView';
import { ApprovalsView } from './views/ApprovalsView';
import { PolicyStudioView } from './views/PolicyStudioView';

/** Root: gate on a WSF session, then render the shell + product-area routes. */
export default function App() {
  const { session } = useAuth();

  if (!session) {
    return <LoginView />;
  }

  return (
    <AppShell>
      <Routes>
        <Route path="/" element={<OverviewView />} />
        <Route path="/routing" element={<RoutingView />} />
        <Route path="/audit" element={<AuditView />} />
        <Route path="/approvals" element={<ApprovalsView />} />
        <Route path="/policy" element={<PolicyStudioView />} />
        <Route path="*" element={<Navigate to="/" replace />} />
      </Routes>
    </AppShell>
  );
}
