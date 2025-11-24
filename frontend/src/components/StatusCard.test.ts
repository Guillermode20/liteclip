import { render, screen } from '@testing-library/svelte'
import { describe, it, expect } from 'vitest'
import StatusCard from './StatusCard.svelte'
import type { StatusMessageType } from '../types'

describe('StatusCard', () => {
  it('renders with default values', () => {
    render(StatusCard)
    
    const statusCard = document.querySelector('.status-card')
    expect(statusCard).toBeInTheDocument()
    expect(statusCard).toHaveClass('status-processing')
    
    const icon = document.querySelector('.status-icon svg')
    expect(icon).toBeInTheDocument()
    
    const message = screen.getByText('')
    expect(message).toBeInTheDocument()
  })

  it('displays custom message', () => {
    render(StatusCard, { message: 'Processing video...' })
    
    expect(screen.getByText('Processing video...')).toBeInTheDocument()
  })

  it('applies correct CSS class for processing type', () => {
    render(StatusCard, { type: 'processing' as StatusMessageType })
    
    const statusCard = document.querySelector('.status-card')
    expect(statusCard).toHaveClass('status-processing')
  })

  it('applies correct CSS class for success type', () => {
    render(StatusCard, { type: 'success' as StatusMessageType })
    
    const statusCard = document.querySelector('.status-card')
    expect(statusCard).toHaveClass('status-success')
  })

  it('applies correct CSS class for error type', () => {
    render(StatusCard, { type: 'error' as StatusMessageType })
    
    const statusCard = document.querySelector('.status-card')
    expect(statusCard).toHaveClass('status-error')
  })

  it('shows processing icon for processing type', () => {
    render(StatusCard, { type: 'processing' as StatusMessageType })
    
    const icon = document.querySelector('.status-icon svg')
    expect(icon).toBeInTheDocument()
    
    // Check for circle and polyline elements (clock icon)
    const circle = icon?.querySelector('circle')
    const polyline = icon?.querySelector('polyline')
    expect(circle).toBeInTheDocument()
    expect(polyline).toBeInTheDocument()
  })

  it('shows success icon for success type', () => {
    render(StatusCard, { type: 'success' as StatusMessageType })
    
    const icon = document.querySelector('.status-icon svg')
    expect(icon).toBeInTheDocument()
    
    // Check for polyline element (checkmark icon)
    const polyline = icon?.querySelector('polyline')
    expect(polyline).toBeInTheDocument()
  })

  it('shows error icon for error type', () => {
    render(StatusCard, { type: 'error' as StatusMessageType })
    
    const icon = document.querySelector('.status-icon svg')
    expect(icon).toBeInTheDocument()
    
    // Check for circle and line elements (X icon)
    const circle = icon?.querySelector('circle')
    const lines = icon?.querySelectorAll('line')
    expect(circle).toBeInTheDocument()
    expect(lines).toHaveLength(2)
  })

  it('displays message with both icon and text', () => {
    render(StatusCard, { 
      message: 'Compression completed successfully!', 
      type: 'success' as StatusMessageType 
    })
    
    expect(screen.getByText('Compression completed successfully!')).toBeInTheDocument()
    
    const icon = document.querySelector('.status-icon svg')
    expect(icon).toBeInTheDocument()
    
    const statusCard = document.querySelector('.status-card')
    expect(statusCard).toHaveClass('status-success')
  })
})
