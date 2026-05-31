import { useState, useCallback } from 'react';
import { TitleBar, SessionList, MainPanel, RightPanel, StatusBar } from './components/layout';
import { MarketPage } from './components/inspiration/MarketPage';

function App() {
  const [showMarket, setShowMarket] = useState(false);
  const [, setPendingInputContent] = useState('');

  const handleSendToSession = useCallback((content: string) => {
    setPendingInputContent(content);
    setShowMarket(false);
    // The InputBar/MainPanel will need to pick this up via store or ref
    // For now, we set it in sessionStorage as a bridge
    sessionStorage.setItem('pilotdesk_pending_input', content);
  }, []);

  return (
    <div
      className="h-screen flex flex-col overflow-hidden"
      style={{ backgroundColor: 'var(--bg-primary)', color: 'var(--text-primary)' }}
    >
      <TitleBar />
      {showMarket ? (
        <MarketPage onBack={() => setShowMarket(false)} onSendToSession={handleSendToSession} />
      ) : (
        <div className="flex-1 flex overflow-hidden relative">
          <SessionList />
          <MainPanel />
          <RightPanel />
        </div>
      )}
      <StatusBar />
    </div>
  );
}

export default App;
