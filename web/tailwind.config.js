/** @type {import('tailwindcss').Config} */
export default {
  darkMode: 'class',
  content: ['./src/**/*.{html,js,svelte,ts}'],
  theme: {
    extend: {
      colors: {
        // status colours used in the package/build tables and status dots
        paur: {
          queued: '#a3a3a3',
          running: '#3b82f6',
          success: '#16a34a',
          failed: '#dc2626',
          cancelled: '#6b7280'
        }
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
