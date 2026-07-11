/** @type {import('tailwindcss').Config} */
export default {
  darkMode: 'class',
  content: ['./src/**/*.{html,js,svelte,ts}'],
  safelist: [
    // badge/status-dot variants are interpolated in StatusBadge.svelte
    { pattern: /badge-(queued|running|success|failed|cancelled)/ },
    { pattern: /status-dot-(queued|running|success|failed|cancelled)/ }
  ],
  theme: {
    extend: {
      colors: {
        vercel: {
          page: '#0a0a0b',
          card: '#111113',
          'card-hover': '#18181b',
          soft: '#18181b',
          elevated: '#1c1c1f',
          hairline: '#1f1f23',
          'hairline-strong': '#3f3f46',
          ink: '#fafafa',
          body: '#a1a1aa',
          mute: '#71717a',
          accent: '#0070f3',
          'accent-soft': 'rgba(0, 112, 243, 0.06)',
          success: '#22c55e',
          error: '#ef4444',
          warning: '#f59e0b'
        },
        // status colours used in the package/build tables and status dots
        paur: {
          queued: '#a3a3a3',
          running: '#3b82f6',
          success: '#16a34a',
          failed: '#dc2626',
          cancelled: '#6b7280'
        }
      },
      fontFamily: {
        sans: ['system-ui', '-apple-system', 'BlinkMacSystemFont', 'Segoe UI', 'Roboto', 'Helvetica Neue', 'Arial', 'sans-serif'],
        mono: ['ui-monospace', 'SFMono-Regular', 'SF Mono', 'Menlo', 'Consolas', 'monospace']
      },
      keyframes: {
        'pulse-dot': {
          '0%, 100%': { opacity: '1', transform: 'scale(1)' },
          '50%': { opacity: '0.45', transform: 'scale(0.82)' }
        },
        'indeterminate-bar': {
          '0%': { transform: 'translateX(-100%)' },
          '100%': { transform: 'translateX(400%)' }
        },
        shimmer: {
          '0%': { backgroundPosition: '-200% 0' },
          '100%': { backgroundPosition: '200% 0' }
        }
      },
      animation: {
        'pulse-dot': 'pulse-dot 1.4s ease-in-out infinite',
        'indeterminate-bar': 'indeterminate-bar 1.4s ease-in-out infinite',
        shimmer: 'shimmer 2s linear infinite'
      }
    }
  },
  plugins: []
};
