import { describe, it, expect, vi, beforeEach, afterAll } from 'vitest'
import { render, screen } from '@testing-library/svelte'
import StatusCard from '../components/StatusCard.svelte'
import type { StatusMessageType } from '../types'

// Mock console methods to test error logging
const originalConsoleError = console.error
const originalConsoleWarn = console.warn

describe('Error Handling in UI', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    console.error = vi.fn()
    console.warn = vi.fn()
  })

  afterAll(() => {
    console.error = originalConsoleError
    console.warn = originalConsoleWarn
  })

  describe('FFmpeg Status Errors', () => {
    it('displays FFmpeg missing error', () => {
      render(StatusCard, { 
        message: 'FFmpeg not found. Please wait while it downloads...', 
        type: 'error' as StatusMessageType 
      })
      
      expect(screen.getByText('FFmpeg not found. Please wait while it downloads...')).toBeInTheDocument()
      
      const statusCard = document.querySelector('.status-card')
      expect(statusCard).toHaveClass('status-error')
      
      // Should show error icon
      const icon = document.querySelector('.status-icon svg')
      expect(icon).toBeInTheDocument()
    })

    it('displays FFmpeg download progress', () => {
      render(StatusCard, { 
        message: 'Downloading FFmpeg... 75%', 
        type: 'processing' as StatusMessageType 
      })
      
      expect(screen.getByText('Downloading FFmpeg... 75%')).toBeInTheDocument()
      
      const statusCard = document.querySelector('.status-card')
      expect(statusCard).toHaveClass('status-processing')
    })

    it('displays FFmpeg ready status', () => {
      render(StatusCard, { 
        message: 'FFmpeg ready for video processing', 
        type: 'success' as StatusMessageType 
      })
      
      expect(screen.getByText('FFmpeg ready for video processing')).toBeInTheDocument()
      
      const statusCard = document.querySelector('.status-card')
      expect(statusCard).toHaveClass('status-success')
    })

    it('displays FFmpeg download failed error', () => {
      render(StatusCard, { 
        message: 'Failed to download FFmpeg. Please check your internet connection.', 
        type: 'error' as StatusMessageType 
      })
      
      expect(screen.getByText('Failed to download FFmpeg. Please check your internet connection.')).toBeInTheDocument()
      
      const statusCard = document.querySelector('.status-card')
      expect(statusCard).toHaveClass('status-error')
    })
  })

  describe('File Size Errors', () => {
    it('displays oversized file error', () => {
      render(StatusCard, { 
        message: 'File too large. Maximum size is 2GB.', 
        type: 'error' as StatusMessageType 
      })
      
      expect(screen.getByText('File too large. Maximum size is 2GB.')).toBeInTheDocument()
      
      const statusCard = document.querySelector('.status-card')
      expect(statusCard).toHaveClass('status-error')
    })

    it('displays empty file error', () => {
      render(StatusCard, { 
        message: 'File is empty or corrupted.', 
        type: 'error' as StatusMessageType 
      })
      
      expect(screen.getByText('File is empty or corrupted.')).toBeInTheDocument()
      
      const statusCard = document.querySelector('.status-card')
      expect(statusCard).toHaveClass('status-error')
    })

    it('displays file not found error', () => {
      render(StatusCard, { 
        message: 'File not found or cannot be accessed.', 
        type: 'error' as StatusMessageType 
      })
      
      expect(screen.getByText('File not found or cannot be accessed.')).toBeInTheDocument()
      
      const statusCard = document.querySelector('.status-card')
      expect(statusCard).toHaveClass('status-error')
    })
  })

  describe('Invalid Format Errors', () => {
    it('displays unsupported format error', () => {
      render(StatusCard, { 
        message: 'Unsupported video format. Please use MP4, AVI, MOV, or WebM.', 
        type: 'error' as StatusMessageType 
      })
      
      expect(screen.getByText('Unsupported video format. Please use MP4, AVI, MOV, or WebM.')).toBeInTheDocument()
      
      const statusCard = document.querySelector('.status-card')
      expect(statusCard).toHaveClass('status-error')
    })

    it('displays corrupted file error', () => {
      render(StatusCard, { 
        message: 'Video file appears to be corrupted or invalid.', 
        type: 'error' as StatusMessageType 
      })
      
      expect(screen.getByText('Video file appears to be corrupted or invalid.')).toBeInTheDocument()
      
      const statusCard = document.querySelector('.status-card')
      expect(statusCard).toHaveClass('status-error')
    })

    it('displays no video stream error', () => {
      render(StatusCard, { 
        message: 'No video stream found in the file.', 
        type: 'error' as StatusMessageType 
      })
      
      expect(screen.getByText('No video stream found in the file.')).toBeInTheDocument()
      
      const statusCard = document.querySelector('.status-card')
      expect(statusCard).toHaveClass('status-error')
    })
  })

  describe('Compression Errors', () => {
    it('displays compression failed error', () => {
      render(StatusCard, { 
        message: 'Compression failed. Please try again with different settings.', 
        type: 'error' as StatusMessageType 
      })
      
      expect(screen.getByText('Compression failed. Please try again with different settings.')).toBeInTheDocument()
      
      const statusCard = document.querySelector('.status-card')
      expect(statusCard).toHaveClass('status-error')
    })

    it('displays out of memory error', () => {
      render(StatusCard, { 
        message: 'Out of memory. Try reducing the video resolution or closing other applications.', 
        type: 'error' as StatusMessageType 
      })
      
      expect(screen.getByText('Out of memory. Try reducing the video resolution or closing other applications.')).toBeInTheDocument()
      
      const statusCard = document.querySelector('.status-card')
      expect(statusCard).toHaveClass('status-error')
    })

    it('displays encoding timeout error', () => {
      render(StatusCard, { 
        message: 'Encoding timed out. The video may be too complex or your system too slow.', 
        type: 'error' as StatusMessageType 
      })
      
      expect(screen.getByText('Encoding timed out. The video may be too complex or your system too slow.')).toBeInTheDocument()
      
      const statusCard = document.querySelector('.status-card')
      expect(statusCard).toHaveClass('status-error')
    })
  })

  describe('Network and Server Errors', () => {
    it('displays connection error', () => {
      render(StatusCard, { 
        message: 'Cannot connect to the compression service. Please restart the application.', 
        type: 'error' as StatusMessageType 
      })
      
      expect(screen.getByText('Cannot connect to the compression service. Please restart the application.')).toBeInTheDocument()
      
      const statusCard = document.querySelector('.status-card')
      expect(statusCard).toHaveClass('status-error')
    })

    it('displays server error', () => {
      render(StatusCard, { 
        message: 'Server error occurred. Please try again.', 
        type: 'error' as StatusMessageType 
      })
      
      expect(screen.getByText('Server error occurred. Please try again.')).toBeInTheDocument()
      
      const statusCard = document.querySelector('.status-card')
      expect(statusCard).toHaveClass('status-error')
    })

    it('displays queue full error', () => {
      render(StatusCard, { 
        message: 'Compression queue is full. Please wait a moment and try again.', 
        type: 'error' as StatusMessageType 
      })
      
      expect(screen.getByText('Compression queue is full. Please wait a moment and try again.')).toBeInTheDocument()
      
      const statusCard = document.querySelector('.status-card')
      expect(statusCard).toHaveClass('status-error')
    })
  })

  describe('Success States', () => {
    it('displays compression success', () => {
      render(StatusCard, { 
        message: 'Compression completed successfully!', 
        type: 'success' as StatusMessageType 
      })
      
      expect(screen.getByText('Compression completed successfully!')).toBeInTheDocument()
      
      const statusCard = document.querySelector('.status-card')
      expect(statusCard).toHaveClass('status-success')
    })

    it('displays file download ready', () => {
      render(StatusCard, { 
        message: 'Your compressed video is ready for download.', 
        type: 'success' as StatusMessageType 
      })
      
      expect(screen.getByText('Your compressed video is ready for download.')).toBeInTheDocument()
      
      const statusCard = document.querySelector('.status-card')
      expect(statusCard).toHaveClass('status-success')
    })
  })

  describe('Processing States', () => {
    it('displays processing message', () => {
      render(StatusCard, { 
        message: 'Analyzing video...', 
        type: 'processing' as StatusMessageType 
      })
      
      expect(screen.getByText('Analyzing video...')).toBeInTheDocument()
      
      const statusCard = document.querySelector('.status-card')
      expect(statusCard).toHaveClass('status-processing')
    })

    it('displays compression progress', () => {
      render(StatusCard, { 
        message: 'Compressing video... 45% complete', 
        type: 'processing' as StatusMessageType 
      })
      
      expect(screen.getByText('Compressing video... 45% complete')).toBeInTheDocument()
      
      const statusCard = document.querySelector('.status-card')
      expect(statusCard).toHaveClass('status-processing')
    })
  })

  describe('Error Recovery', () => {
    it('displays retry suggestion', () => {
      render(StatusCard, { 
        message: 'Operation failed. Please try again.', 
        type: 'error' as StatusMessageType 
      })
      
      expect(screen.getByText('Operation failed. Please try again.')).toBeInTheDocument()
      
      const statusCard = document.querySelector('.status-card')
      expect(statusCard).toHaveClass('status-error')
    })

    it('displays fallback suggestion', () => {
      render(StatusCard, { 
        message: 'Advanced encoding failed. Trying basic encoding...', 
        type: 'processing' as StatusMessageType 
      })
      
      expect(screen.getByText('Advanced encoding failed. Trying basic encoding...')).toBeInTheDocument()
      
      const statusCard = document.querySelector('.status-card')
      expect(statusCard).toHaveClass('status-processing')
    })
  })
})
