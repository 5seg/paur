/** @type {import('tailwindcss').Config} */
export default {
  content: ['./src/**/*.{html,js,svelte,ts}'],
  theme: {
    extend: {
      colors: {
        // status colours used in the package/build tables
        paur: {
          queued: '#a3a3a3',
          running: '#3b82f6',
          success: '#16a34a',
          failed: '#dc2626',
          cancelled: '#6b7280'
        }
      }
    }
  },
  plugins: []
};
