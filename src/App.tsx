import { useState, useCallback } from 'react';
import { TitleBar, SessionList, MainPanel, RightPanel, StatusBar } from './components/layout';
import { MarketPage } from './components/inspiration/MarketPage';
import { EnvPage } from './pages/EnvPage';

type PageView = 'main' | 'market' | 'env';

function App() {
  const [currentPage, setCurrentPage] = useState<PageView>('main');
  const [, setPendingInputContent] = useState('');

  const handleSendToSession = useCallback((content: string) => {
    setPendingInputContent(content);
    setCurrentPage('main');
    sessionStorage.setItem('pilotdesk_pending_input', content);
  }, []);

  return (
    <div
      className="h-screen flex flex-col overflow-hidden"
      style={{ backgroundColor: 'var(--bg-primary)', color: 'var(--text-primary)' }}
    >
      <TitleBar onOpenEnv={() => setCurrentPage('env')} onOpenMarket={() => setCurrentPage('market')} />
      {currentPage === 'market' ? (
        <MarketPage onBack={() => setCurrentPage('main')} onSendToSession={handleSendToSession} />
      ) : currentPage === 'env' ? (
        <EnvPage onBack={() => setCurrentPage('main')} />
      ) : (
        <div className="flex-1 flex overflow-hidden relative">
          <SessionList />
          <MainPanel />
          <RightPanel />
        </div>
      )}
      <StatusBar onOpenEnv={() => setCurrentPage('env')} />
    </div>
  );
}

export default App;
