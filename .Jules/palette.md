## 2023-10-25 - Added Loading State to Login Button
**Learning:** Adding a small animation during async operations (like logging in) provides immediate visual feedback, significantly improving the perceived responsiveness of the app and preventing user confusion or double-clicking. Utilizing existing icons (`Loader2` from `lucide-react`) combined with Tailwind's `animate-spin` utility ensures consistency without needing custom CSS.
**Action:** Consistently apply loading states to any primary submit buttons executing asynchronous tasks across the dashboard.
