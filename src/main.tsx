import React from 'react';
import ReactDOM from 'react-dom/client';
import App from './App';
import './i18n';
import './design/tokens.css';
import './design/fonts.css';
import './styles.css';
import { installCth } from './cth';
import { installOfficeStore } from './store/store';

installCth();
installOfficeStore();

ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>
);
