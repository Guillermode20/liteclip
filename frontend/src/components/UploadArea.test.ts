import { render, screen, fireEvent } from '@testing-library/svelte'
import { describe, it, expect, vi, beforeEach } from 'vitest'
import UploadArea from './UploadArea.svelte'
import { writable } from 'svelte/store'

// Mock the video store
const mockVideoStore = {
  setFile: vi.fn()
}

vi.mock('../stores/video', () => ({
  videoStore: {
    setFile: mockVideoStore.setFile
  }
}))

describe('UploadArea', () => {
  beforeEach(() => {
    vi.clearAllMocks()
  })

  it('renders upload prompt by default', () => {
    render(UploadArea)
    
    expect(screen.getByText('// upload_video')).toBeInTheDocument()
    expect(screen.getByText('Drop video or click to select')).toBeInTheDocument()
    
    const uploadArea = screen.getByRole('region', { name: 'Video upload area' })
    expect(uploadArea).toBeInTheDocument()
    expect(uploadArea).not.toHaveClass('has-video')
  })

  it('shows ready state when file is selected and has controls', () => {
    const mockFile = new File(['test'], 'test.mp4', { type: 'video/mp4' })
    render(UploadArea, { 
      selectedFile: mockFile, 
      hasControls: true,
      fileInfo: 'test.mp4 • 10.2 MB • 00:30'
    })
    
    expect(screen.getByText('Video ready for editing')).toBeInTheDocument()
    expect(screen.getByText('Drop another file to replace it')).toBeInTheDocument()
    expect(screen.getByText('test.mp4 • 10.2 MB • 00:30')).toBeInTheDocument()
    
    const uploadArea = screen.getByRole('region', { name: 'Video upload area' })
    expect(uploadArea).toHaveClass('has-video')
  })

  it('does not show ready state when file is selected but no controls', () => {
    const mockFile = new File(['test'], 'test.mp4', { type: 'video/mp4' })
    render(UploadArea, { 
      selectedFile: mockFile, 
      hasControls: false
    })
    
    expect(screen.getByText('Drop video or click to select')).toBeInTheDocument()
    expect(screen.queryByText('Video ready for editing')).not.toBeInTheDocument()
    
    const uploadArea = screen.getByRole('region', { name: 'Video upload area' })
    expect(uploadArea).not.toHaveClass('has-video')
  })

  it('triggers file input when upload area is clicked', () => {
    const { component } = render(UploadArea)
    
    const uploadPrompt = screen.getByText('Drop video or click to select').closest('div')
    expect(uploadPrompt).toBeInTheDocument()
    
    // Mock the file input reference
    const mockClick = vi.fn()
    component.fileInputRef = { click: mockClick }
    
    uploadPrompt && fireEvent.click(uploadPrompt)
    expect(mockClick).toHaveBeenCalled()
  })

  it('triggers file input when Enter key is pressed', () => {
    const { component } = render(UploadArea)
    
    const uploadPrompt = screen.getByText('Drop video or click to select').closest('div')
    expect(uploadPrompt).toBeInTheDocument()
    
    // Mock the file input reference
    const mockClick = vi.fn()
    component.fileInputRef = { click: mockClick }
    
    uploadPrompt && fireEvent.keyDown(uploadPrompt, { key: 'Enter' })
    expect(mockClick).toHaveBeenCalled()
  })

  it('triggers file input when Space key is pressed', () => {
    const { component } = render(UploadArea)
    
    const uploadPrompt = screen.getByText('Drop video or click to select').closest('div')
    expect(uploadPrompt).toBeInTheDocument()
    
    // Mock the file input reference
    const mockClick = vi.fn()
    component.fileInputRef = { click: mockClick }
    
    uploadPrompt && fireEvent.keyDown(uploadPrompt, { key: ' ' })
    expect(mockClick).toHaveBeenCalled()
  })

  it('handles file selection from input change', async () => {
    const mockFile = new File(['test'], 'test.mp4', { type: 'video/mp4' })
    const { component } = render(UploadArea)
    
    // Create a mock file input
    const mockFileInput = {
      files: [mockFile],
      value: ''
    }
    component.fileInputRef = mockFileInput
    
    // Mock the event target
    const mockEvent = {
      target: mockFileInput
    }
    
    // Call the handler directly
    await component.handleFileInputChange(mockEvent)
    
    expect(mockVideoStore.setFile).toHaveBeenCalledWith(mockFile)
  })

  it('handles drag and drop', async () => {
    const mockFile = new File(['test'], 'test.mp4', { type: 'video/mp4' })
    const { component } = render(UploadArea)
    
    // Create mock drag event
    const mockDropEvent = {
      preventDefault: vi.fn(),
      dataTransfer: {
        files: [mockFile]
      }
    }
    
    // Call the handler directly
    await component.handleDrop(mockDropEvent)
    
    expect(mockDropEvent.preventDefault).toHaveBeenCalled()
    expect(mockVideoStore.setFile).toHaveBeenCalledWith(mockFile)
  })

  it('handles drag over correctly', async () => {
    const { component } = render(UploadArea)
    
    const mockDragOverEvent = {
      preventDefault: vi.fn()
    }
    
    await component.handleDragOver(mockDragOverEvent)
    
    expect(mockDragOverEvent.preventDefault).toHaveBeenCalled()
    expect(component.isDragover).toBe(true)
  })

  it('handles drag leave correctly', async () => {
    const { component } = render(UploadArea)
    
    // Set dragover state first
    component.isDragover = true
    
    await component.handleDragLeave()
    
    expect(component.isDragover).toBe(false)
  })

  it('applies dragover class during drag over', () => {
    const { component } = render(UploadArea)
    
    component.isDragover = true
    
    const uploadArea = screen.getByRole('region', { name: 'Video upload area' })
    expect(uploadArea).toHaveClass('dragover')
  })

  it('dispatches fileSelected event when file is selected', async () => {
    const mockFile = new File(['test'], 'test.mp4', { type: 'video/mp4' })
    const { component } = render(UploadArea)
    
    // Mock the dispatch function
    const mockDispatch = vi.fn()
    component.dispatch = mockDispatch
    
    // Call file selection handler
    const mockFiles = [mockFile]
    await component.handleFileSelection(mockFiles)
    
    expect(mockVideoStore.setFile).toHaveBeenCalledWith(mockFile)
    expect(mockDispatch).toHaveBeenCalledWith('fileSelected', { file: mockFile })
  })

  it('handles empty file selection gracefully', async () => {
    const { component } = render(UploadArea)
    
    const mockDispatch = vi.fn()
    component.dispatch = mockDispatch
    
    await component.handleFileSelection(null)
    
    expect(mockVideoStore.setFile).not.toHaveBeenCalled()
    expect(mockDispatch).not.toHaveBeenCalled()
  })

  it('handles empty file list gracefully', async () => {
    const { component } = render(UploadArea)
    
    const mockDispatch = vi.fn()
    component.dispatch = mockDispatch
    
    await component.handleFileSelection([])
    
    expect(mockVideoStore.setFile).not.toHaveBeenCalled()
    expect(mockDispatch).not.toHaveBeenCalled()
  })
})
