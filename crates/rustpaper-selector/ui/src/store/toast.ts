import { create } from "zustand";

interface ToastState {
  message: string | null;
  show: (message: string) => void;
}

let timer: ReturnType<typeof setTimeout> | undefined;

export const useToast = create<ToastState>((set) => ({
  message: null,
  show: (message) => {
    set({ message });
    clearTimeout(timer);
    timer = setTimeout(() => set({ message: null }), 2000);
  },
}));

/// For stores and event handlers outside React.
export const toast = (message: string) => useToast.getState().show(message);
export const errorText = (e: unknown) => (e instanceof Error ? e.message : String(e));
