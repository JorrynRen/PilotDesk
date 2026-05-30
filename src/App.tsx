import { TitleBar, SessionList, MainPanel, RightPanel, StatusBar } from './components/layout';

function App() {
  return (
    <div
      className="h-screen flex flex-col overflow-hidden"
      style={{ backgroundColor: 'var(--bg-primary)', color: 'var(--text-primary)' }}
    >
      <TitleBar />
      <div className="flex-1 flex overflow-hidden relative">
        <SessionList />
        <MainPanel />
        <RightPanel />
      </div>
      <StatusBar />
    </div>
  );
}

export default App;
